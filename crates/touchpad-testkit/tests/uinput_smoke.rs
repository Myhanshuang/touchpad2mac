#![cfg(target_os = "linux")]

use std::rc::Rc;
use std::thread;
use std::time::Duration;

use touchpad_core::{ContactFrame, ContactState};
use touchpad_linux::sink::FrameSink;
use touchpad_linux::sys::ffi::LinuxSys;
use touchpad_linux::{EvdevRuntime, ProbeVerdict};
use touchpad_testkit::uinput::VirtualTouchpad;

#[derive(Default)]
struct Frames(Vec<ContactFrame>);

impl FrameSink for Frames {
    fn on_frame(&mut self, frame: ContactFrame) {
        self.0.push(frame);
    }
}

#[test]
fn kernel_uinput_to_evdev_runtime_to_type_b_decoder() {
    if std::env::var_os("TOUCHPAD2MAC_RUN_UINPUT").is_none() {
        eprintln!("skipped: set TOUCHPAD2MAC_RUN_UINPUT=1 to run the kernel system test");
        return;
    }
    assert!(VirtualTouchpad::available(), "/dev/uinput is required");
    let device = VirtualTouchpad::create().expect("create uinput touchpad");
    let sys = Rc::new(LinuxSys::new());

    let report = touchpad_linux::probe(&*sys, device.event_path());
    assert!(
        matches!(report.verdict, ProbeVerdict::Candidate { .. }),
        "virtual device was not accepted: {report:?}"
    );

    let mut runtime = EvdevRuntime::open(sys, device.event_path(), Frames::default())
        .expect("open virtual touchpad through real evdev runtime");
    device.three_contacts(200, 300).expect("inject contacts");
    thread::sleep(Duration::from_millis(20));
    runtime.step().expect("decode injected frame");
    device.release_three().expect("inject release");
    thread::sleep(Duration::from_millis(20));
    runtime.step().expect("decode release frame");

    let frames = &runtime.sink_mut().expect("sink remains attached").0;
    assert!(frames.iter().any(|frame| frame.contacts.len() == 3));
    assert!(frames.iter().any(|frame| {
        frame.contacts.len() == 3
            && frame
                .contacts
                .iter()
                .all(|contact| contact.state == ContactState::Ended)
    }));
}
