//! The narrow M10 takeover frame bridge (M10_TASK.md §6).
//!
//! The decoder's [`FrameSink`] callback is **infallible**, while
//! [`ArbiterSink::frame`] is **fallible**. [`TakeoverBridge`] is the narrow
//! adapter between the two for the takeover pipeline:
//!
//! * It wraps the approved M7–M9 [`ArbiterSink`] and forwards every committed
//!   [`ContactFrame`] to it.
//! * It stores the **first** arbiter/output failure and immediately stops
//!   accepting semantic work: every later frame from the same already-read
//!   evdev batch is ignored (and counted), so a mid-batch output rejection
//!   can never produce semantic or wire output after the fault.
//! * The command inspects the stored fault after every runtime step and
//!   begins the ordered shutdown (M10_TASK.md §6: "Do not silently log and
//!   continue, and do not replace the primary fault with a generic decoder
//!   error" — the fault is preserved structurally).
//! * [`TakeoverBridge::release_all`] delegates to the `ArbiterSink`'s
//!   accepted-prefix/faulted cleanup, so cleanup submits exactly the
//!   still-owed releases (button ups, `ScrollEnd`) and the wrapped output
//!   session's own cleanup.
//!
//! The bridge is generic over the output sink (`S: OutputSink`); production
//! code instantiates it with a prepared streaming portal/libei session (see
//! `touchpad-desktop`), tests with a recording or fault-injecting fake.
//!
//! This module is `unsafe`-free.

#![forbid(unsafe_code)]

use touchpad_core::{
    Arbiter, ArbiterConfig, ArbiterSink, ArbiterSinkError, ContactFrame, Monotonic, OutputSink,
};

use crate::sink::FrameSink;

/// The narrow takeover frame bridge: infallible [`FrameSink`] → fallible
/// [`ArbiterSink`], storing the first failure and ignoring later frames.
#[derive(Debug)]
pub struct TakeoverBridge<S: OutputSink> {
    arbiter_sink: ArbiterSink<S>,
    /// The first arbiter/output failure, if any. Once set, no further frame
    /// is forwarded (fail-stop).
    fault: Option<ArbiterSinkError>,
    /// Sticky fail-stop flag: set together with the first fault and **never
    /// cleared by [`TakeoverBridge::take_fault`]** — after a fault the bridge
    /// never accepts semantic work again, even if the coordinator took the
    /// fault out for reporting (the no-late-output rule covers the remainder
    /// of the already-read batch and any later input).
    stopped: bool,
    /// Frames forwarded to the arbiter successfully.
    frames_processed: u64,
    /// Frames ignored after the first fault (from the same already-read
    /// batch; they must never produce output).
    frames_ignored_after_fault: u64,
}

impl<S: OutputSink> TakeoverBridge<S> {
    /// Creates a bridge over an arbiter configured with `config` and an
    /// output sink.
    #[must_use]
    pub fn new(config: ArbiterConfig, sink: S) -> Self {
        Self {
            arbiter_sink: ArbiterSink::new(config, sink),
            fault: None,
            stopped: false,
            frames_processed: 0,
            frames_ignored_after_fault: 0,
        }
    }

    /// Whether a fault was stored (a frame was rejected by the arbiter or the
    /// output sink). When set, no later frame is forwarded.
    #[must_use]
    pub const fn is_faulted(&self) -> bool {
        self.fault.is_some()
    }

    /// Whether the bridge has stopped accepting semantic work (the sticky
    /// fail-stop flag). Differs from [`TakeoverBridge::is_faulted`] only
    /// after [`TakeoverBridge::take_fault`] consumed the fault: the bridge
    /// stays stopped.
    #[must_use]
    pub const fn is_stopped(&self) -> bool {
        self.stopped
    }

    /// The stored fault, if any (borrowed).
    #[must_use]
    pub const fn fault(&self) -> Option<&ArbiterSinkError> {
        self.fault.as_ref()
    }

    /// Takes the stored fault out (the caller reports it as the primary stop
    /// reason). Returns `None` when no fault was stored. The bridge **stays
    /// stopped**: no later frame is forwarded.
    #[must_use]
    pub fn take_fault(&mut self) -> Option<ArbiterSinkError> {
        self.fault.take()
    }

    /// Frames successfully forwarded to the arbiter (for status reporting).
    #[must_use]
    pub const fn frames_processed(&self) -> u64 {
        self.frames_processed
    }

    /// Frames ignored because a fault was already stored (never forwarded).
    #[must_use]
    pub const fn frames_ignored_after_fault(&self) -> u64 {
        self.frames_ignored_after_fault
    }

    /// The underlying arbiter.
    #[must_use]
    pub const fn arbiter(&self) -> &Arbiter {
        self.arbiter_sink.arbiter()
    }

    /// M19 neutral-boundary runtime settings replacement. After a sticky
    /// output fault no semantic work or configuration change is accepted.
    pub fn try_replace_config(&mut self, config: ArbiterConfig) -> bool {
        !self.stopped && self.arbiter_sink.try_replace_config(config)
    }

    /// A mutable reference to the underlying output sink (M10: the takeover
    /// coordinator prepares a streaming output session through this accessor
    /// after the device is open but before any read or grab).
    #[must_use]
    pub fn sink_mut(&mut self) -> &mut S {
        self.arbiter_sink.sink_mut()
    }

    /// Releases all held state through the underlying `ArbiterSink` —
    /// idempotent, retryable, and reports every failed explicit release and
    /// the wrapped sink's cleanup failure structurally
    /// ([`ArbiterSinkError::ReleaseFailed`], review M9 R5).
    pub fn release_all(&mut self) -> Result<(), ArbiterSinkError> {
        self.arbiter_sink.release_all()
    }

    /// Advances time-driven core policy (currently deferred tap release).
    /// Like frame forwarding, the first arbiter/output failure is stored and
    /// the bridge becomes sticky fail-stop; later ticks/frames are ignored by
    /// the coordinator.
    pub fn tick(&mut self, timestamp: Monotonic) -> Result<(), ArbiterSinkError> {
        if self.stopped {
            return Err(self.fault.clone().unwrap_or(ArbiterSinkError::Faulted));
        }
        match self.arbiter_sink.tick(timestamp) {
            Ok(_) => Ok(()),
            Err(error) => {
                self.fault = Some(error.clone());
                self.stopped = true;
                Err(error)
            }
        }
    }

    /// Splits the bridge back into the arbiter and the output sink (the
    /// coordinator uses this after release to observe the session).
    #[must_use]
    pub fn into_parts(self) -> (Arbiter, S) {
        self.arbiter_sink.into_parts()
    }
}

impl<S: OutputSink> FrameSink for TakeoverBridge<S> {
    /// Forwards one committed frame to the arbiter/sink. Infallible: after a
    /// fault is stored, later frames are ignored (never forwarded), which is
    /// the no-late-output rule for the remainder of the already-read batch.
    fn on_frame(&mut self, frame: ContactFrame) {
        if self.stopped {
            self.frames_ignored_after_fault += 1;
            return;
        }
        match self.arbiter_sink.frame(&frame) {
            Ok(_) => self.frames_processed += 1,
            Err(error) => {
                // Store the FIRST failure; later frames (same batch) are
                // ignored above. The fault survives for the coordinator to
                // take and report as the primary stop reason; the sticky
                // `stopped` flag keeps the bridge fail-stop even after the
                // fault is taken.
                self.fault = Some(error);
                self.stopped = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use touchpad_core::{
        Contact, ContactState, LogicalPixels, LogicalPixelsPerMm, Millimeters, Monotonic,
        MouseButton, OutputError, OutputEvent, PhysicalButtons, RecordingSink,
    };

    fn mm(x: f32) -> Millimeters {
        Millimeters::try_new(x).unwrap()
    }

    fn config() -> ArbiterConfig {
        ArbiterConfig::new(mm(1.0), LogicalPixelsPerMm::try_new(10.0).unwrap()).unwrap()
    }

    fn complete(tracking_id: i32, slot: u32, state: ContactState, x: f32, y: f32) -> Contact {
        let mut c = Contact::new(tracking_id, slot, state);
        c.x_mm = Some(mm(x));
        c.y_mm = Some(mm(y));
        c
    }

    fn frame(sequence: u64, ts: u64, contacts: Vec<Contact>) -> ContactFrame {
        ContactFrame {
            monotonic_timestamp: Monotonic::from_nanos(ts),
            sequence,
            discontinuity: false,
            contacts,
            physical_buttons: PhysicalButtons::NONE,
            diagnostics: vec![],
        }
    }

    fn frame_with_left(sequence: u64, ts: u64, contacts: Vec<Contact>, left: bool) -> ContactFrame {
        let mut f = frame(sequence, ts, contacts);
        f.physical_buttons = PhysicalButtons::new(left, false, false);
        f
    }

    fn began(sequence: u64, ts: u64, tracking_id: i32) -> ContactFrame {
        frame(
            sequence,
            ts,
            vec![complete(tracking_id, 0, ContactState::Began, 0.0, 0.0)],
        )
    }

    fn active(sequence: u64, ts: u64, tracking_id: i32, x: f32, y: f32) -> ContactFrame {
        frame(
            sequence,
            ts,
            vec![complete(tracking_id, 0, ContactState::Active, x, y)],
        )
    }

    fn ended(sequence: u64, ts: u64, tracking_id: i32) -> ContactFrame {
        frame(
            sequence,
            ts,
            vec![complete(tracking_id, 0, ContactState::Ended, 2.0, 0.0)],
        )
    }

    /// An output sink with a scripted per-submission outcome: each `submit`
    /// consumes the next scripted result (exhausted → `Ok`). **Accepted**
    /// events are recorded; rejected events are not (mirroring "a failed
    /// submit must leave the sink in a state where the event is not tracked
    /// as delivered").
    struct ScriptedSink {
        script: std::collections::VecDeque<Result<(), OutputError>>,
        submitted: Vec<OutputEvent>,
        releases: usize,
    }

    impl OutputSink for ScriptedSink {
        fn submit(&mut self, event: OutputEvent) -> Result<(), OutputError> {
            match self.script.pop_front().unwrap_or(Ok(())) {
                Ok(()) => {
                    self.submitted.push(event);
                    Ok(())
                }
                Err(_) => Err(OutputError::Rejected(event)),
            }
        }

        fn release_all(&mut self) -> Result<(), OutputError> {
            self.releases += 1;
            Ok(())
        }
    }

    #[test]
    fn forwards_frames_to_the_arbiter_sink_in_order() {
        let mut bridge = TakeoverBridge::new(config(), RecordingSink::new());
        bridge.on_frame(began(1, 1, 7));
        bridge.on_frame(active(2, 2, 7, 2.0, 0.0)); // commits: +10 px move
        bridge.on_frame(ended(3, 3, 7));
        assert!(!bridge.is_faulted());
        assert_eq!(bridge.frames_processed(), 3);
        assert_eq!(bridge.frames_ignored_after_fault(), 0);
        let (_, mut sink) = bridge.into_parts();
        let events = sink.take_events();
        // The first committed movement accounts exactly once for the whole
        // displacement accumulated since the candidate anchor: 2.0 mm ×
        // 10 px/mm = 20 px.
        assert!(events.contains(&OutputEvent::PointerMove {
            dx: LogicalPixels::try_new(20.0).unwrap(),
            dy: LogicalPixels::try_new(0.0).unwrap(),
        }));
    }

    #[test]
    fn first_fault_is_stored_and_later_frames_are_ignored() {
        let mut bridge = TakeoverBridge::new(
            config(),
            ScriptedSink {
                script: [Ok(()), Err(OutputError::Rejected(OutputEvent::ScrollBegin))].into(),
                submitted: vec![],
                releases: 0,
            },
        );
        // Frame 1: one event (nothing for a below-threshold begin) -> no
        // submission. Frame 2: commits and submits a move (first submit).
        // Frame 3: also submits a move (second submit) -> rejected.
        bridge.on_frame(began(1, 1, 7));
        bridge.on_frame(active(2, 2, 7, 2.0, 0.0)); // submit 1: move(+20, 0) — accepted
        bridge.on_frame(active(3, 3, 7, 3.0, 0.0)); // submit 2: move(+10, 0) — rejected -> fault
        assert!(bridge.is_faulted());
        assert!(bridge.is_stopped());
        let fault = bridge.take_fault();
        assert!(
            matches!(
                fault,
                Some(ArbiterSinkError::PartialSubmit {
                    index: 0,
                    accepted_prefix: 0,
                    ..
                })
            ),
            "{fault:?}"
        );
        // Later frames from the same batch are ignored (never forwarded) —
        // even after the fault was taken, the bridge stays stopped.
        bridge.on_frame(active(4, 4, 7, 4.0, 0.0));
        assert_eq!(bridge.frames_ignored_after_fault(), 1);
        assert!(bridge.is_stopped());
        assert!(!bridge.is_faulted(), "the fault was taken for reporting");
        // Release submits exactly the owed state: the accepted down/begin
        // that was not released. Here no button/scroll was accepted, so
        // release only calls the wrapped cleanup.
        bridge.release_all().unwrap();
    }

    #[test]
    fn release_all_submits_exactly_the_owed_releases_after_a_partial_fault() {
        // A sink that rejects the 3rd submission: press + move accepted, the
        // up rejected -> the left down is owed.
        let mut bridge = TakeoverBridge::new(
            config(),
            ScriptedSink {
                script: [
                    Ok(()),
                    Ok(()),
                    Err(OutputError::Rejected(OutputEvent::ScrollBegin)),
                ]
                .into(),
                submitted: vec![],
                releases: 0,
            },
        );
        // Physical left press while the finger is below the threshold: the
        // frame carries the button edge (submission 1: ButtonDown(Left)).
        bridge.on_frame(frame_with_left(
            1,
            1,
            vec![complete(7, 0, ContactState::Began, 0.0, 0.0)],
            true,
        ));
        // Move while held (submission 2: PointerMove accepted).
        bridge.on_frame(frame_with_left(
            2,
            2,
            vec![complete(7, 0, ContactState::Active, 2.0, 0.0)],
            true,
        ));
        // Physical left release (submission 3: ButtonUp(Left) rejected).
        bridge.on_frame(frame_with_left(
            3,
            3,
            vec![complete(7, 0, ContactState::Active, 2.0, 0.0)],
            false,
        ));
        assert!(bridge.is_faulted(), "fault after the rejected up");
        // Later frames ignored.
        bridge.on_frame(ended(4, 4, 7));
        assert_eq!(bridge.frames_ignored_after_fault(), 1);

        // Cleanup: the accepted ButtonDown(Left) is owed -> exactly one
        // ButtonUp(Left) through the sink, then the wrapped release_all.
        let (arbiter, sink) = {
            let result = bridge.release_all();
            assert!(result.is_ok(), "{result:?}");
            bridge.into_parts()
        };
        assert!(!arbiter.is_left_held());
        let ups: Vec<_> = sink
            .submitted
            .iter()
            .filter(|e| matches!(e, OutputEvent::ButtonUp(MouseButton::Left)))
            .cloned()
            .collect();
        assert_eq!(
            ups.len(),
            1,
            "exactly the owed left up: {:?}",
            sink.submitted
        );
        assert_eq!(sink.releases, 1, "the wrapped cleanup runs once");
    }

    #[test]
    fn sink_mut_exposes_the_output_sink() {
        let mut bridge = TakeoverBridge::new(config(), RecordingSink::new());
        let sink = bridge.sink_mut();
        sink.submit(OutputEvent::ScrollBegin).unwrap();
        assert_eq!(sink.len(), 1);
    }
}
