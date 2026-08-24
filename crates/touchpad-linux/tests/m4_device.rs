//! M4 end-to-end tests: device enumeration → candidate pick → runtime open →
//! read/decode → controlled shutdown, entirely through the public API and
//! the mockable [`Sys`] seam. No real device is ever opened or grabbed.

use std::path::PathBuf;
use std::rc::Rc;

use touchpad_core::ContactState;
use touchpad_linux::sys::mock::{MockCall, MockDevice, MockSys};
use touchpad_linux::{
    enumerate, pick_candidate, EvdevRuntime, ProbeVerdict, RecordingFrameSink, RuntimeError,
    RuntimePhase, CLOCK_MONOTONIC,
};

fn ev_bytes(sec: i64, usec: i64, event_type: u16, code: u16, value: i32) -> Vec<u8> {
    touchpad_linux::encode_input_event(sec, usec, event_type, code, value)
}

/// A candidate touchpad whose read stream begins one contact and then loses
/// continuity, with a kernel snapshot that restores it.
fn touchpad_with_dropped_stream() -> MockDevice {
    use touchpad_linux::{
        ABS_MT_POSITION_X, ABS_MT_POSITION_Y, ABS_MT_TRACKING_ID, EV_ABS, EV_SYN, SYN_DROPPED,
        SYN_REPORT,
    };
    let mut device = MockDevice::touchpad("E2E Pad", 8);
    let mut batch = vec![
        ev_bytes(1, 0, EV_ABS, touchpad_linux::ABS_MT_SLOT, 0),
        ev_bytes(1, 0, EV_ABS, ABS_MT_TRACKING_ID, 5),
        ev_bytes(1, 0, EV_ABS, ABS_MT_POSITION_X, 100),
        ev_bytes(1, 0, EV_ABS, ABS_MT_POSITION_Y, 50),
        ev_bytes(1, 0, EV_SYN, SYN_REPORT, 0),
        ev_bytes(1, 0, EV_SYN, SYN_DROPPED, 0),
        ev_bytes(1, 0, EV_SYN, SYN_REPORT, 0),
    ];
    device.push_raw(
        batch
            .split_off(0)
            .into_iter()
            .flatten()
            .collect::<Vec<u8>>(),
    );
    // The snapshot sees slot 0 active again with a fresh tracking id.
    device.set_mt_slots(ABS_MT_TRACKING_ID, vec![9, -1, -1, -1, -1, -1, -1, -1]);
    device.set_mt_slots(ABS_MT_POSITION_X, vec![120, 0, 0, 0, 0, 0, 0, 0]);
    device.set_mt_slots(ABS_MT_POSITION_Y, vec![60, 0, 0, 0, 0, 0, 0, 0]);
    device
}

#[test]
fn enumerate_pick_open_step_shutdown_end_to_end() {
    let sys = Rc::new(MockSys::new());
    sys.set_dir_entries(vec![
        PathBuf::from("/dev/input/event0"), // touchscreen: rejected
        PathBuf::from("/dev/input/event1"), // touchpad: candidate
    ]);
    let mut touchscreen = MockDevice::touchpad("Touchscreen", 8);
    touchscreen.add_prop(touchpad_linux::INPUT_PROP_DIRECT);
    touchscreen.prop_bits[touchpad_linux::INPUT_PROP_POINTER as usize / 8] &=
        !(1 << (touchpad_linux::INPUT_PROP_POINTER % 8));
    sys.add_device(PathBuf::from("/dev/input/event0"), touchscreen);
    sys.add_device(
        PathBuf::from("/dev/input/event1"),
        touchpad_with_dropped_stream(),
    );

    let reports = enumerate(&*sys).unwrap();
    assert_eq!(reports.len(), 2);
    assert!(matches!(reports[0].verdict, ProbeVerdict::Rejected { .. }));
    assert!(matches!(reports[1].verdict, ProbeVerdict::Candidate { .. }));
    assert_eq!(pick_candidate(&reports), Some(1));

    // Open the candidate, then grab it explicitly (M5 review R2: `open`
    // never grabs; the grab is a separate checked runtime step).
    let sys_rc: Rc<dyn touchpad_linux::sys::Sys> = sys.clone();
    let mut runtime = EvdevRuntime::open(
        sys_rc,
        &PathBuf::from("/dev/input/event1"),
        RecordingFrameSink::new(),
    )
    .unwrap();
    runtime.grab().unwrap();

    // One step processes the whole stream: a normal frame, a SYN_DROPPED,
    // and a successful resync (discontinuity frame).
    runtime.step().unwrap();

    // Controlled shutdown; the grab was released.
    let report = runtime.shutdown();
    assert!(report.ungrab.as_ref().unwrap().is_ok());
    assert!(report.close.as_ref().unwrap().is_ok());
    assert_eq!(report.phase, RuntimePhase::Stopped);

    // The frames were published through the same decoder path as live input.
    let frames = runtime.into_sink().frames().to_vec();
    assert_eq!(frames.len(), 2);
    assert!(!frames[0].discontinuity);
    assert_eq!(frames[0].contacts[0].state, ContactState::Began);
    assert!(frames[1].discontinuity);
    assert_eq!(frames[1].contacts[0].tracking_id, 9);
    assert_eq!(frames[1].contacts[0].state, ContactState::Began);
    assert_eq!(frames[0].sequence, 1);
    assert_eq!(frames[1].sequence, 2);
    let fd = match sys
        .log()
        .iter()
        .find(|c| matches!(c, touchpad_linux::sys::mock::MockCall::Grab(_, true)))
    {
        Some(touchpad_linux::sys::mock::MockCall::Grab(fd, true)) => *fd,
        _ => panic!("no grab recorded"),
    };
    assert_eq!(
        sys.count(
            |call| matches!(call, touchpad_linux::sys::mock::MockCall::Grab(f, false) if *f == fd)
        ),
        1
    );
    assert_eq!(
        sys.count(|call| matches!(call, touchpad_linux::sys::mock::MockCall::Close(f) if *f == fd)),
        1
    );
    // M4 review R1: the runtime selected CLOCK_MONOTONIC on the same fd,
    // before the grab.
    assert_eq!(
        sys.count(|call| matches!(
            call,
            touchpad_linux::sys::mock::MockCall::ClockId(f, clock)
                if *f == fd && *clock == CLOCK_MONOTONIC
        )),
        1
    );
    let log = sys.log();
    let clock = log
        .iter()
        .position(|call| matches!(call, MockCall::ClockId(f, _) if *f == fd))
        .expect("clock ioctl");
    let grab = log
        .iter()
        .position(|call| matches!(call, MockCall::Grab(f, true) if *f == fd))
        .expect("grab");
    assert!(clock < grab, "clock must be selected before the grab");
}

/// M4 review R6 end-to-end: one read batch carries a normal frame, a
/// `SYN_DROPPED`/recovery boundary, and multiple post-boundary tracking-id
/// lifecycles that predate the snapshot. The runtime must drain them: only
/// the pre-drop frame and the snapshot's discontinuity frame are published.
#[test]
fn resync_drains_post_boundary_lifecycles_end_to_end() {
    use touchpad_linux::{
        ABS_MT_POSITION_X, ABS_MT_POSITION_Y, ABS_MT_SLOT, ABS_MT_TRACKING_ID, EV_ABS, EV_SYN,
        SYN_DROPPED, SYN_REPORT,
    };
    let sys = Rc::new(MockSys::new());
    let path = PathBuf::from("/dev/input/event1");
    let mut device = MockDevice::touchpad("Drain Pad", 8);
    let mut batch = vec![
        ev_bytes(1, 0, EV_ABS, ABS_MT_SLOT, 0),
        ev_bytes(1, 0, EV_ABS, ABS_MT_TRACKING_ID, 5),
        ev_bytes(1, 0, EV_ABS, ABS_MT_POSITION_X, 100),
        ev_bytes(1, 0, EV_ABS, ABS_MT_POSITION_Y, 50),
        ev_bytes(1, 0, EV_SYN, SYN_REPORT, 0),
        ev_bytes(1, 0, EV_SYN, SYN_DROPPED, 0),
        ev_bytes(1, 0, EV_SYN, SYN_REPORT, 0),
        // Post-boundary stale lifecycles (predate the snapshot ioctl):
        // slot 0: tid 6 begin/end, tid 7 begin; slot 1: tid 8 begin.
        ev_bytes(1, 0, EV_ABS, ABS_MT_SLOT, 0),
        ev_bytes(1, 0, EV_ABS, ABS_MT_TRACKING_ID, 6),
        ev_bytes(1, 0, EV_ABS, ABS_MT_TRACKING_ID, -1),
        ev_bytes(1, 0, EV_ABS, ABS_MT_TRACKING_ID, 7),
        ev_bytes(1, 0, EV_ABS, ABS_MT_POSITION_X, 200),
        ev_bytes(1, 0, EV_ABS, ABS_MT_POSITION_Y, 100),
        ev_bytes(1, 0, EV_ABS, ABS_MT_SLOT, 1),
        ev_bytes(1, 0, EV_ABS, ABS_MT_TRACKING_ID, 8),
        ev_bytes(1, 0, EV_ABS, ABS_MT_POSITION_X, 300),
        ev_bytes(1, 0, EV_ABS, ABS_MT_POSITION_Y, 150),
        ev_bytes(1, 0, EV_SYN, SYN_REPORT, 0),
    ];
    device.push_raw(batch.split_off(0).into_iter().flatten().collect());
    // The snapshot sees slot 0 with tid 7 and slot 1 empty.
    device.set_mt_slots(ABS_MT_TRACKING_ID, vec![7, -1, -1, -1, -1, -1, -1, -1]);
    device.set_mt_slots(ABS_MT_POSITION_X, vec![200, 0, 0, 0, 0, 0, 0, 0]);
    device.set_mt_slots(ABS_MT_POSITION_Y, vec![100, 0, 0, 0, 0, 0, 0, 0]);
    sys.add_device(&path, device);

    let sys_rc: Rc<dyn touchpad_linux::sys::Sys> = sys.clone();
    let mut runtime = EvdevRuntime::open(sys_rc, &path, RecordingFrameSink::new()).unwrap();
    let fed = runtime.step().unwrap();
    // 5 pre-drop + SYN_DROPPED + recovery SYN_REPORT = 7 events fed; the
    // post-boundary lifecycles are drained.
    assert_eq!(
        fed, 7,
        "feeding must stop right after the recovery boundary"
    );

    let frames = runtime.into_sink().frames().to_vec();
    assert_eq!(
        frames.len(),
        2,
        "only pre-drop + discontinuity frames: {frames:#?}"
    );
    assert!(!frames[0].discontinuity);
    assert_eq!(frames[0].contacts[0].tracking_id, 5);
    assert!(frames[1].discontinuity);
    assert_eq!(frames[1].contacts.len(), 1);
    assert_eq!(frames[1].contacts[0].tracking_id, 7);
    for frame in &frames {
        for contact in &frame.contacts {
            assert!(
                !matches!(contact.tracking_id, 6 | 8),
                "stale drained lifecycle {} must not be emitted",
                contact.tracking_id
            );
        }
    }
}

#[test]
fn no_devices_is_a_clean_empty_result() {
    let sys = Rc::new(MockSys::new());
    sys.set_dir_entries(vec![]);
    let reports = enumerate(&*sys).unwrap();
    assert!(reports.is_empty());
    assert_eq!(pick_candidate(&reports), None);
}

#[test]
fn missing_input_dir_is_an_actionable_error() {
    let sys = Rc::new(MockSys::new());
    sys.set_read_dir_error(touchpad_linux::sys::mock::MockFailure::NotFound);
    let err = enumerate(&*sys).unwrap_err();
    assert!(err.to_string().contains("/dev/input"), "{err}");
}

#[test]
fn opening_a_rejected_device_fails_with_reasons() {
    let sys = Rc::new(MockSys::new());
    let path = PathBuf::from("/dev/input/event0");
    let mut touchscreen = MockDevice::touchpad("Touchscreen", 8);
    touchscreen.add_prop(touchpad_linux::INPUT_PROP_DIRECT);
    touchscreen.prop_bits[touchpad_linux::INPUT_PROP_POINTER as usize / 8] &=
        !(1 << (touchpad_linux::INPUT_PROP_POINTER % 8));
    sys.add_device(&path, touchscreen);
    let sys_rc: Rc<dyn touchpad_linux::sys::Sys> = sys.clone();
    let err = match EvdevRuntime::open(sys_rc, &path, RecordingFrameSink::new()) {
        Err(err) => err,
        Ok(_) => panic!("expected open of a direct-touch device to fail"),
    };
    assert!(
        matches!(
            err,
            RuntimeError::Open(touchpad_linux::OpenError::NotCandidate { .. })
        ),
        "{err:?}"
    );
    assert!(err.to_string().contains("INPUT_PROP_DIRECT"), "{err}");
}
