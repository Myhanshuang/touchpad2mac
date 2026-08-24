#![forbid(unsafe_code)]
//! The fixed, bounded `--emit` test pattern (M6 required outcome 4).
//!
//! Real desktop emission is an explicit, separate opt-in
//! (`touchpadctl output-probe --emit`), preceded by a visible warning and
//! countdown, and the emitted sequence is this **fixed, short, bounded
//! pattern** — never a free-form stream. The pattern covers the M6 output
//! contract: small/medium/large relative pointer deltas (for the reviewer's
//! A/B displacement measurement), a primary click, a pixel-precise smooth
//! scroll lifecycle, and a secondary click — each step gated on the
//! negotiated capability. Tests run the pattern only through fake
//! transports and never emit real desktop input.

use std::time::Duration;

use touchpad_core::{LogicalPixels, MouseButton, OutputEvent, OutputSink};

use crate::capabilities::{Capability, OutputCapabilities};
use crate::desktop::EmitDriver;
use crate::error::DesktopOutputError;

/// The hard upper bound on wire events the pattern can emit. The fixed
/// pattern emits far fewer; the bound is a defensive invariant so a
/// programming error can never turn `--emit` into an unbounded stream.
pub const MAX_PATTERN_EVENTS: usize = 16;

/// One step of the fixed pattern.
#[derive(Debug, Clone, PartialEq)]
pub struct PatternStep {
    /// Human-readable description (also printed during `--emit`).
    pub description: &'static str,
    /// The typed events of this step, in order.
    pub events: Vec<OutputEvent>,
    /// The capability the step needs; steps whose capability was not
    /// negotiated are skipped (and reported), never silently faked.
    pub required: Capability,
    /// Pause after the step, so the reviewer can observe/measure each
    /// sample on the real desktop.
    pub pause_after: Duration,
}

fn px(value: f32) -> LogicalPixels {
    LogicalPixels::try_new(value).expect("pattern values are finite")
}

fn move_event(dx: f32, dy: f32) -> OutputEvent {
    OutputEvent::PointerMove {
        dx: px(dx),
        dy: px(dy),
    }
}

fn click(button: MouseButton) -> Vec<OutputEvent> {
    vec![
        OutputEvent::ButtonDown(button),
        OutputEvent::ButtonUp(button),
    ]
}

fn scroll_lifecycle(deltas: &[(f32, f32)]) -> Vec<OutputEvent> {
    let mut events = vec![OutputEvent::ScrollBegin];
    for (dx, dy) in deltas {
        events.push(OutputEvent::ScrollDelta {
            dx: px(*dx),
            dy: px(*dy),
        });
    }
    events.push(OutputEvent::ScrollEnd);
    events
}

/// The fixed `--emit` pattern. Deterministic; the same sequence every run.
#[must_use]
pub fn pattern() -> Vec<PatternStep> {
    vec![
        PatternStep {
            description: "relative pointer move +10px x-axis (small delta)",
            events: vec![move_event(10.0, 0.0)],
            required: Capability::RelativePointer,
            pause_after: Duration::from_millis(600),
        },
        PatternStep {
            description: "relative pointer move +50px x-axis (medium delta)",
            events: vec![move_event(50.0, 0.0)],
            required: Capability::RelativePointer,
            pause_after: Duration::from_millis(600),
        },
        PatternStep {
            description: "relative pointer move +200px x-axis (large delta)",
            events: vec![move_event(200.0, 0.0)],
            required: Capability::RelativePointer,
            pause_after: Duration::from_millis(600),
        },
        PatternStep {
            description: "primary button click (BTN_LEFT down/up)",
            events: click(MouseButton::Left),
            required: Capability::PrimaryButton,
            pause_after: Duration::from_millis(400),
        },
        PatternStep {
            description: "pixel-precise smooth scroll (begin, -120px, -240px, end)",
            events: scroll_lifecycle(&[(0.0, -120.0), (0.0, -240.0)]),
            required: Capability::PixelScroll,
            pause_after: Duration::from_millis(600),
        },
        PatternStep {
            description: "secondary button click (BTN_RIGHT down/up)",
            events: click(MouseButton::Right),
            required: Capability::SecondaryButton,
            pause_after: Duration::from_millis(400),
        },
    ]
}

/// The outcome of running the pattern.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EmitOutcome {
    /// Number of steps actually emitted.
    pub steps_emitted: usize,
    /// Descriptions of steps skipped because their capability was not
    /// negotiated (reported, never silently faked).
    pub skipped: Vec<String>,
    /// Number of wire events submitted.
    pub wire_events: usize,
    /// The capabilities the emission ran with.
    pub capabilities: OutputCapabilities,
}

/// Runs the fixed pattern through an [`OutputSink`], honouring the
/// negotiated capabilities and the driver's cancellation between steps.
/// `release_all` remains the caller's responsibility (it is invoked by the
/// emit driver in [`crate::desktop`] on every path).
pub fn run_pattern(
    sink: &mut dyn OutputSink,
    capabilities: OutputCapabilities,
    driver: &mut EmitDriver<'_>,
) -> Result<EmitOutcome, DesktopOutputError> {
    let mut outcome = EmitOutcome {
        capabilities,
        ..EmitOutcome::default()
    };
    for step in pattern() {
        if (driver.cancelled)() {
            return Err(DesktopOutputError::Cancelled);
        }
        if !capabilities.supports(step.required) {
            outcome.skipped.push(step.description.to_string());
            (driver.progress)(&format!(
                "    skipped: {} (capability not negotiated)",
                step.description
            ));
            continue;
        }
        (driver.progress)(&format!("    {}", step.description));
        for event in &step.events {
            outcome.wire_events += 1;
            if outcome.wire_events > MAX_PATTERN_EVENTS {
                return Err(DesktopOutputError::Internal(format!(
                    "pattern exceeded the {MAX_PATTERN_EVENTS}-event bound"
                )));
            }
            sink.submit(event.clone())
                .map_err(|error| DesktopOutputError::SendFailed(format!("{event:?}: {error}")))?;
        }
        outcome.steps_emitted += 1;
        (driver.sleeper)(step.pause_after);
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::desktop::DesktopOutput;
    use crate::fake::FakeDesktopOutput;
    use crate::sink::BIND_CAPABILITY_BITS;

    /// Builds a driver with no-op hooks and hands it to `f`, so the
    /// closures' borrows stay inside the call.
    fn with_noop_driver<R>(f: impl FnOnce(&mut EmitDriver<'_>) -> R) -> R {
        let mut sleeper = |_: Duration| {};
        let mut progress = |_: &str| {};
        let cancelled = || false;
        let mut driver = EmitDriver {
            sleeper: &mut sleeper,
            progress: &mut progress,
            cancelled: &cancelled,
        };
        f(&mut driver)
    }

    /// A sink recording submissions (no real emission ever).
    #[derive(Default)]
    struct RecordingSink {
        events: Vec<OutputEvent>,
    }

    impl OutputSink for RecordingSink {
        fn submit(&mut self, event: OutputEvent) -> Result<(), touchpad_core::OutputError> {
            self.events.push(event);
            Ok(())
        }

        fn release_all(&mut self) -> Result<(), touchpad_core::OutputError> {
            Ok(())
        }
    }

    #[test]
    fn pattern_is_fixed_and_bounded() {
        let steps = pattern();
        // The pattern is the same every call.
        assert_eq!(steps, pattern());
        let total: usize = steps.iter().map(|step| step.events.len()).sum();
        assert!(
            total <= MAX_PATTERN_EVENTS,
            "{total} > {MAX_PATTERN_EVENTS}"
        );
        // The pattern exercises every M6 capability.
        let required: Vec<Capability> = steps.iter().map(|step| step.required).collect();
        for cap in [
            Capability::RelativePointer,
            Capability::PrimaryButton,
            Capability::SecondaryButton,
            Capability::PixelScroll,
        ] {
            assert!(required.contains(&cap), "missing {cap:?}");
        }
    }

    #[test]
    fn run_pattern_emits_all_steps_with_full_capabilities() {
        let caps = OutputCapabilities::from_device_capability_bits(BIND_CAPABILITY_BITS);
        let mut sink = RecordingSink::default();
        let outcome = with_noop_driver(|driver| run_pattern(&mut sink, caps, driver)).unwrap();
        assert_eq!(outcome.steps_emitted, pattern().len());
        assert!(outcome.skipped.is_empty());
        assert_eq!(outcome.wire_events, sink.events.len());
        assert!(sink.events.len() <= MAX_PATTERN_EVENTS);
    }

    #[test]
    fn run_pattern_skips_missing_capabilities_and_reports_them() {
        // Pointer-only: no buttons, no scroll.
        let caps = OutputCapabilities::from_device_capability_bits(1 << 0);
        let mut sink = RecordingSink::default();
        let outcome = with_noop_driver(|driver| run_pattern(&mut sink, caps, driver)).unwrap();
        assert_eq!(outcome.steps_emitted, 3); // only the three moves
        assert_eq!(outcome.skipped.len(), 3); // clicks + scroll reported
        assert!(sink
            .events
            .iter()
            .all(|event| matches!(event, OutputEvent::PointerMove { .. })));
    }

    #[test]
    fn run_pattern_honours_cancellation_between_steps() {
        let caps = OutputCapabilities::from_device_capability_bits(BIND_CAPABILITY_BITS);
        let cancelled = std::cell::Cell::new(false);
        let mut sleeper = |_: Duration| {};
        let mut progress = |_: &str| {};
        let cancelled_fn = || cancelled.get();
        let mut driver = EmitDriver {
            sleeper: &mut sleeper,
            progress: &mut progress,
            cancelled: &cancelled_fn,
        };
        let mut sink = RecordingSink::default();
        // Cancel before the first step.
        cancelled.set(true);
        let error = run_pattern(&mut sink, caps, &mut driver).unwrap_err();
        assert_eq!(error, DesktopOutputError::Cancelled);
        assert!(sink.events.is_empty());
    }

    #[test]
    fn fake_desktop_output_never_emits_and_records_the_call() {
        let mut output = FakeDesktopOutput::available();
        let report = output.probe();
        assert!(report.session_bus.is_ok());
        let outcome = with_noop_driver(|driver| output.emit_pattern(driver)).unwrap();
        assert!(output.emit_called);
        assert_eq!(outcome, EmitOutcome::default());
    }
}
