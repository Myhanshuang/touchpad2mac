//! Testable Windows compatibility output.
//!
//! This path intentionally calls itself *compatibility* output. `SendInput`
//! can represent relative mouse motion, buttons and wheel deltas, but wheel
//! messages are not equivalent to libei pixel-precise scroll. The sink is
//! useful for Windows bring-up and overlay experiments; it must not be used
//! to claim the existing `PixelScroll` capability.

#![forbid(unsafe_code)]

use touchpad_core::{LogicalPixels, MouseButton, OutputError, OutputEvent, OutputSink};

use crate::WindowsError;

/// Minimal OS seam used by [`WindowsOutputSink`].
pub trait WindowsOutputApi {
    /// Injects one relative mouse motion.
    fn relative_move(&mut self, dx: i32, dy: i32) -> Result<(), WindowsError>;
    /// Injects one mouse button edge.
    fn button(&mut self, button: MouseButton, down: bool) -> Result<(), WindowsError>;
    /// Injects one wheel delta. Vertical positive values follow Win32
    /// `MOUSEEVENTF_WHEEL`; horizontal positive values follow
    /// `MOUSEEVENTF_HWHEEL`.
    fn wheel(&mut self, horizontal: bool, delta: i32) -> Result<(), WindowsError>;
}

#[derive(Clone, Copy, Debug, Default)]
struct HeldButtons {
    left: bool,
    right: bool,
    middle: bool,
    x1: bool,
    x2: bool,
}

impl HeldButtons {
    fn get(self, button: MouseButton) -> Option<bool> {
        match button {
            MouseButton::Left => Some(self.left),
            MouseButton::Right => Some(self.right),
            MouseButton::Middle => Some(self.middle),
            MouseButton::Other(1) => Some(self.x1),
            MouseButton::Other(2) => Some(self.x2),
            MouseButton::Other(_) => None,
            _ => None,
        }
    }

    fn set(&mut self, button: MouseButton, value: bool) -> bool {
        match button {
            MouseButton::Left => self.left = value,
            MouseButton::Right => self.right = value,
            MouseButton::Middle => self.middle = value,
            MouseButton::Other(1) => self.x1 = value,
            MouseButton::Other(2) => self.x2 = value,
            MouseButton::Other(_) => return false,
            _ => return false,
        }
        true
    }
}

/// Stateful compatibility sink over a mockable Win32 output seam.
pub struct WindowsOutputSink<A: WindowsOutputApi> {
    api: A,
    held: HeldButtons,
    wheel_x_residue: f64,
    wheel_y_residue: f64,
    wheel_units_per_logical_pixel: f64,
}

impl<A: WindowsOutputApi> WindowsOutputSink<A> {
    /// Creates a sink. `wheel_units_per_logical_pixel` is deliberately
    /// explicit because Win32 wheel units are not logical pixels.
    pub fn new(api: A, wheel_units_per_logical_pixel: f64) -> Result<Self, WindowsError> {
        if !wheel_units_per_logical_pixel.is_finite() || wheel_units_per_logical_pixel <= 0.0 {
            return Err(WindowsError::Unsupported(
                "a finite positive wheel-units-per-logical-pixel scale".into(),
            ));
        }
        Ok(Self {
            api,
            held: HeldButtons::default(),
            wheel_x_residue: 0.0,
            wheel_y_residue: 0.0,
            wheel_units_per_logical_pixel,
        })
    }

    /// Returns the wrapped API, primarily for deterministic tests.
    pub fn into_inner(self) -> A {
        self.api
    }

    fn submit_button(&mut self, button: MouseButton, down: bool) -> Result<(), OutputError> {
        let Some(current) = self.held.get(button) else {
            return Err(OutputError::Rejected(if down {
                OutputEvent::ButtonDown(button)
            } else {
                OutputEvent::ButtonUp(button)
            }));
        };
        if current == down {
            return Ok(());
        }
        self.api.button(button, down).map_err(map_output_error)?;
        let _ = self.held.set(button, down);
        Ok(())
    }

    fn submit_scroll(&mut self, dx: LogicalPixels, dy: LogicalPixels) -> Result<(), OutputError> {
        self.wheel_x_residue += f64::from(dx.as_px()) * self.wheel_units_per_logical_pixel;
        self.wheel_y_residue += f64::from(dy.as_px()) * self.wheel_units_per_logical_pixel;

        let x = drain_integral(&mut self.wheel_x_residue);
        let y = drain_integral(&mut self.wheel_y_residue);
        if x != 0 {
            self.api.wheel(true, x).map_err(map_output_error)?;
        }
        if y != 0 {
            self.api.wheel(false, y).map_err(map_output_error)?;
        }
        Ok(())
    }
}

fn drain_integral(value: &mut f64) -> i32 {
    let integral = value.trunc();
    let clamped = integral.clamp(f64::from(i32::MIN), f64::from(i32::MAX));
    let out = clamped as i32;
    *value -= f64::from(out);
    out
}

fn rounded_i32(value: f32) -> i32 {
    f64::from(value)
        .round()
        .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}

fn map_output_error(error: WindowsError) -> OutputError {
    OutputError::Io(error.to_string())
}

impl<A: WindowsOutputApi> OutputSink for WindowsOutputSink<A> {
    fn submit(&mut self, event: OutputEvent) -> Result<(), OutputError> {
        match event {
            OutputEvent::PointerMove { dx, dy } => {
                let dx = rounded_i32(dx.as_px());
                let dy = rounded_i32(dy.as_px());
                if dx == 0 && dy == 0 {
                    return Ok(());
                }
                self.api.relative_move(dx, dy).map_err(map_output_error)
            }
            OutputEvent::ButtonDown(button) => self.submit_button(button, true),
            OutputEvent::ButtonUp(button) => self.submit_button(button, false),
            OutputEvent::ScrollBegin | OutputEvent::ScrollEnd => Ok(()),
            OutputEvent::ScrollDelta { dx, dy } => self.submit_scroll(dx, dy),
            unsupported => Err(OutputError::Rejected(unsupported)),
        }
    }

    fn release_all(&mut self) -> Result<(), OutputError> {
        let mut first_error = None;
        for button in [
            MouseButton::Left,
            MouseButton::Right,
            MouseButton::Middle,
            MouseButton::Other(1),
            MouseButton::Other(2),
        ] {
            if self.held.get(button) == Some(true) {
                if let Err(error) = self.api.button(button, false) {
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                } else {
                    let _ = self.held.set(button, false);
                }
            }
        }
        self.wheel_x_residue = 0.0;
        self.wheel_y_residue = 0.0;
        match first_error {
            Some(error) => Err(OutputError::Fatal(error.to_string())),
            None => Ok(()),
        }
    }
}

/// Real Windows compatibility output. Construction is side-effect free; each
/// submitted event calls `SendInput` on Windows and returns `NotWindows`
/// elsewhere.
pub struct RealWindowsOutput {
    #[cfg(target_os = "windows")]
    inner: WindowsOutputSink<crate::win32::Win32OutputApi>,
}

impl RealWindowsOutput {
    /// Creates the real compatibility sink. A wheel scale of `1.0` means one
    /// logical-pixel unit is forwarded as one Win32 wheel-data unit; this is
    /// intentionally not advertised as pixel-precise scrolling.
    pub fn new() -> Result<Self, WindowsError> {
        #[cfg(target_os = "windows")]
        {
            Ok(Self {
                inner: WindowsOutputSink::new(crate::win32::Win32OutputApi, 1.0)?,
            })
        }
        #[cfg(not(target_os = "windows"))]
        {
            Err(WindowsError::NotWindows)
        }
    }
}

impl OutputSink for RealWindowsOutput {
    fn submit(&mut self, event: OutputEvent) -> Result<(), OutputError> {
        #[cfg(target_os = "windows")]
        {
            self.inner.submit(event)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = event;
            Err(OutputError::Unavailable(
                "Windows compatibility output is unavailable on this platform".into(),
            ))
        }
    }

    fn release_all(&mut self) -> Result<(), OutputError> {
        #[cfg(target_os = "windows")]
        {
            self.inner.release_all()
        }
        #[cfg(not(target_os = "windows"))]
        {
            Ok(())
        }
    }
}

/// Result of the bounded Windows output probe pattern.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EmitProbeOutcome {
    /// Number of semantic events accepted by the sink.
    pub events_emitted: usize,
}

/// Emits a short fixed Windows compatibility pattern through an arbitrary
/// sink. This is separated from the real Win32 API so tests never move the
/// actual pointer.
pub fn emit_fixed_probe_pattern(
    sink: &mut dyn OutputSink,
) -> Result<EmitProbeOutcome, OutputError> {
    fn px(value: f32) -> LogicalPixels {
        LogicalPixels::try_new(value).expect("probe constants are finite")
    }
    let events = [
        OutputEvent::PointerMove {
            dx: px(10.0),
            dy: px(0.0),
        },
        OutputEvent::PointerMove {
            dx: px(50.0),
            dy: px(0.0),
        },
        OutputEvent::PointerMove {
            dx: px(200.0),
            dy: px(0.0),
        },
        OutputEvent::ButtonDown(MouseButton::Left),
        OutputEvent::ButtonUp(MouseButton::Left),
        OutputEvent::ScrollBegin,
        OutputEvent::ScrollDelta {
            dx: px(0.0),
            dy: px(120.0),
        },
        OutputEvent::ScrollEnd,
        OutputEvent::ButtonDown(MouseButton::Right),
        OutputEvent::ButtonUp(MouseButton::Right),
    ];
    let mut outcome = EmitProbeOutcome::default();
    for event in events {
        sink.submit(event)?;
        outcome.events_emitted += 1;
    }
    sink.release_all()?;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum Call {
        Move(i32, i32),
        Button(MouseButton, bool),
        Wheel(bool, i32),
    }

    #[derive(Clone, Default)]
    struct FakeApi {
        calls: Rc<RefCell<Vec<Call>>>,
    }

    impl WindowsOutputApi for FakeApi {
        fn relative_move(&mut self, dx: i32, dy: i32) -> Result<(), WindowsError> {
            self.calls.borrow_mut().push(Call::Move(dx, dy));
            Ok(())
        }

        fn button(&mut self, button: MouseButton, down: bool) -> Result<(), WindowsError> {
            self.calls.borrow_mut().push(Call::Button(button, down));
            Ok(())
        }

        fn wheel(&mut self, horizontal: bool, delta: i32) -> Result<(), WindowsError> {
            self.calls.borrow_mut().push(Call::Wheel(horizontal, delta));
            Ok(())
        }
    }

    fn px(value: f32) -> LogicalPixels {
        LogicalPixels::try_new(value).unwrap()
    }

    #[test]
    fn semantic_order_and_release_are_deterministic() {
        let api = FakeApi::default();
        let calls = Rc::clone(&api.calls);
        let mut sink = WindowsOutputSink::new(api, 1.0).unwrap();
        sink.submit(OutputEvent::PointerMove {
            dx: px(4.4),
            dy: px(-2.6),
        })
        .unwrap();
        sink.submit(OutputEvent::ButtonDown(MouseButton::Left))
            .unwrap();
        sink.submit(OutputEvent::ScrollDelta {
            dx: px(1.5),
            dy: px(-2.25),
        })
        .unwrap();
        sink.release_all().unwrap();
        assert_eq!(
            &*calls.borrow(),
            &[
                Call::Move(4, -3),
                Call::Button(MouseButton::Left, true),
                Call::Wheel(true, 1),
                Call::Wheel(false, -2),
                Call::Button(MouseButton::Left, false),
            ]
        );
    }

    #[test]
    fn wheel_fraction_is_carried_between_frames() {
        let api = FakeApi::default();
        let calls = Rc::clone(&api.calls);
        let mut sink = WindowsOutputSink::new(api, 0.25).unwrap();
        for _ in 0..3 {
            sink.submit(OutputEvent::ScrollDelta {
                dx: px(0.0),
                dy: px(1.0),
            })
            .unwrap();
        }
        assert!(calls.borrow().is_empty());
        sink.submit(OutputEvent::ScrollDelta {
            dx: px(0.0),
            dy: px(1.0),
        })
        .unwrap();
        assert_eq!(&*calls.borrow(), &[Call::Wheel(false, 1)]);
    }

    #[test]
    fn unsupported_semantic_actions_fail_explicitly() {
        let mut sink = WindowsOutputSink::new(FakeApi::default(), 1.0).unwrap();
        let event = OutputEvent::DesktopAction(touchpad_core::DesktopAction::ShowDesktop);
        assert_eq!(
            sink.submit(event.clone()),
            Err(OutputError::Rejected(event))
        );
    }

    #[test]
    fn fixed_probe_is_bounded_and_complete() {
        let api = FakeApi::default();
        let calls = Rc::clone(&api.calls);
        let mut sink = WindowsOutputSink::new(api, 1.0).unwrap();
        let outcome = emit_fixed_probe_pattern(&mut sink).unwrap();
        assert_eq!(outcome.events_emitted, 10);
        assert!(calls.borrow().len() < 16);
    }
}
