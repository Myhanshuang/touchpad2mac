//! M11 one-finger pointer-fidelity stage (`m11-fidelity-v1`, M11_TASK.md).
//!
//! This module is the **pure, platform-independent** fidelity stage for
//! already normalized millimeter input. It applies only to **committed
//! one-finger pointer motion** (M11_TASK.md §2/§5): raw counts never enter
//! it, candidate/tap/scroll ownership stays on raw normalized millimeters
//! before gain or dead-zone logic, and the stage never owns an output sink.
//!
//! # What the stage provides
//!
//! * a signed radial jitter dead-zone (`dead_zone_radius_mm`);
//! * a monotonic, time-domain velocity estimate (EMA with
//!   `velocity_tau`, rate-independent in exact arithmetic);
//! * a continuous bounded smoothstep gain curve (`gain_x0`/`gain_x1`,
//!   `min_gain`/`max_gain`);
//! * an explicit tracking-speed multiplier (`tracking_speed`);
//! * finite scaled pixel deltas that the Arbiter quantizes through the
//!   **existing** per-axis subpixel remainder (no second accumulator).
//!
//! # State machine (M11_TASK.md §7 — authoritative)
//!
//! The stage keeps two separate signed displacement accumulators:
//!
//! * `P` — millimeters waiting for dead-zone release;
//! * `V_pending` + `t_acc` — millimeters waiting for a valid velocity
//!   sample (positive elapsed time).
//!
//! First call: anchor the timestamp, fold the whole accepted displacement
//! into `P`, **do not** put it in the velocity numerator, and evaluate the
//! dead zone at the initial filtered velocity `0` (hence `min_gain`).
//!
//! Duplicate timestamp (`dt == 0`): fold into both `P` and `V_pending`, add
//! zero to `t_acc`, do **not** divide or update velocity, and do **not**
//! evaluate the dead zone — `P` merely accumulates and is not flushed. The
//! duplicate displacement enters the next positive-`dt` velocity sample
//! exactly once.
//!
//! Positive elapsed time (`0 < dt < long_gap`): fold into both, accumulate
//! the time, compute `s = hypot(V) / t_acc`, `alpha = 1 - exp(-t_acc/tau)`,
//! update `v = alpha*s + (1 - alpha)*v_prev`, clear `V`/`t_acc`, advance the
//! anchor, then evaluate the dead zone.
//!
//! Long gap (`dt >= long_gap`, inclusive, checked **before** folding the
//! gap-crossing delta): discard the displacement, clear `P`/`V`/`t_acc`/`v`,
//! re-anchor to the gap-crossing frame, and return [`FidelityOutcome::Reanchored`]
//! (a normal policy outcome, not an error; the pixel remainder survives).
//!
//! # Errors
//!
//! [`FidelityError`] covers only runtime fidelity arithmetic that becomes
//! non-finite or overflows; the Arbiter maps it fail-closed to
//! [`ArbiterError::NonFinite`](crate::ArbiterError::NonFinite). Dead-zone
//! hold, reset, and re-anchor are normal outcomes. Timestamp/sequence
//! regression remains the Arbiter's own error, checked before this stage
//! runs.
//!
//! # Configuration
//!
//! [`FidelityConfig`] is fully typed and validated at construction
//! ([`FidelityConfigError`]); no value is read from KDE/libinput.

use std::time::Duration;

use crate::time::Monotonic;
use crate::units::LogicalPixelsPerMm;

/// Failure of [`FidelityConfig::new`]: a documented field or field
/// relationship is invalid. Construction-only — runtime arithmetic failures
/// are [`FidelityError`].
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum FidelityConfigError {
    /// The dead-zone radius must be finite and strictly positive.
    #[error("fidelity dead zone radius must be finite and strictly positive, got {0}")]
    DeadZoneRadius(f64),
    /// The velocity time constant must be strictly positive.
    #[error("fidelity velocity time constant must be strictly positive")]
    ZeroVelocityTau,
    /// The long gap must be strictly positive (the boundary is inclusive).
    #[error("fidelity long gap must be strictly positive")]
    ZeroLongGap,
    /// The gain curve's lower velocity must be finite and strictly positive.
    #[error("fidelity gain curve x0 must be finite and strictly positive, got {0}")]
    GainX0(f64),
    /// The gain curve's upper velocity must be finite and strictly greater
    /// than `x0`.
    #[error("fidelity gain curve x1 must be finite and strictly greater than x0, got {0}")]
    GainX1(f64),
    /// The minimum gain must be finite, strictly positive, and not exceed the
    /// maximum.
    #[error("fidelity minimum gain must be finite and strictly positive, got {0}")]
    MinGain(f64),
    /// The maximum gain must be finite and not below the minimum.
    #[error("fidelity maximum gain must be finite and not below the minimum gain, got {0}")]
    MaxGain(f64),
    /// The tracking-speed multiplier must be finite and strictly positive.
    #[error("fidelity tracking speed must be finite and strictly positive, got {0}")]
    TrackingSpeed(f64),
}

/// The typed, validated fidelity configuration (M11_TASK.md §10).
///
/// `base_px_per_mm` is the existing validated scale type
/// ([`LogicalPixelsPerMm`]); every other numeric value is validated here.
/// There is deliberately no way to build an invalid configuration.
#[derive(Clone, Debug, PartialEq)]
pub struct FidelityConfig {
    dead_zone_radius_mm: f64,
    velocity_tau: Duration,
    long_gap: Duration,
    gain_x0_mm_per_s: f64,
    gain_x1_mm_per_s: f64,
    min_gain: f64,
    max_gain: f64,
    base_px_per_mm: LogicalPixelsPerMm,
    tracking_speed: f64,
}

impl FidelityConfig {
    /// Creates a validated fidelity configuration.
    ///
    /// # Errors
    ///
    /// Returns the matching [`FidelityConfigError`] variant when a field is
    /// non-finite/non-positive or a field relationship is impossible. The
    /// base scale is already validated by [`LogicalPixelsPerMm`].
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        dead_zone_radius_mm: f64,
        velocity_tau: Duration,
        long_gap: Duration,
        gain_x0_mm_per_s: f64,
        gain_x1_mm_per_s: f64,
        min_gain: f64,
        max_gain: f64,
        base_px_per_mm: LogicalPixelsPerMm,
        tracking_speed: f64,
    ) -> Result<Self, FidelityConfigError> {
        if !dead_zone_radius_mm.is_finite() || dead_zone_radius_mm <= 0.0 {
            return Err(FidelityConfigError::DeadZoneRadius(dead_zone_radius_mm));
        }
        if velocity_tau.is_zero() {
            return Err(FidelityConfigError::ZeroVelocityTau);
        }
        if long_gap.is_zero() {
            return Err(FidelityConfigError::ZeroLongGap);
        }
        if !gain_x0_mm_per_s.is_finite() || gain_x0_mm_per_s <= 0.0 {
            return Err(FidelityConfigError::GainX0(gain_x0_mm_per_s));
        }
        if !gain_x1_mm_per_s.is_finite() || gain_x1_mm_per_s <= gain_x0_mm_per_s {
            return Err(FidelityConfigError::GainX1(gain_x1_mm_per_s));
        }
        if !min_gain.is_finite() || min_gain <= 0.0 {
            return Err(FidelityConfigError::MinGain(min_gain));
        }
        if !max_gain.is_finite() || max_gain < min_gain {
            return Err(FidelityConfigError::MaxGain(max_gain));
        }
        if !tracking_speed.is_finite() || tracking_speed <= 0.0 {
            return Err(FidelityConfigError::TrackingSpeed(tracking_speed));
        }
        Ok(Self {
            dead_zone_radius_mm,
            velocity_tau,
            long_gap,
            gain_x0_mm_per_s,
            gain_x1_mm_per_s,
            min_gain,
            max_gain,
            base_px_per_mm,
            tracking_speed,
        })
    }

    /// The signed radial jitter dead-zone radius in millimeters.
    #[must_use]
    pub const fn dead_zone_radius_mm(&self) -> f64 {
        self.dead_zone_radius_mm
    }

    /// The velocity estimate's time constant.
    #[must_use]
    pub const fn velocity_tau(&self) -> Duration {
        self.velocity_tau
    }

    /// The inclusive long gap: a `dt >= long_gap` discards the gap-crossing
    /// displacement and re-anchors.
    #[must_use]
    pub const fn long_gap(&self) -> Duration {
        self.long_gap
    }

    /// The lower velocity (mm/s) of the smoothstep gain curve.
    #[must_use]
    pub const fn gain_x0_mm_per_s(&self) -> f64 {
        self.gain_x0_mm_per_s
    }

    /// The upper velocity (mm/s) of the smoothstep gain curve.
    #[must_use]
    pub const fn gain_x1_mm_per_s(&self) -> f64 {
        self.gain_x1_mm_per_s
    }

    /// The minimum gain (at or below `gain_x0`).
    #[must_use]
    pub const fn min_gain(&self) -> f64 {
        self.min_gain
    }

    /// The maximum gain (at or above `gain_x1`).
    #[must_use]
    pub const fn max_gain(&self) -> f64 {
        self.max_gain
    }

    /// The base logical-pixels-per-millimeter scale (existing validated type).
    #[must_use]
    pub const fn base_px_per_mm(&self) -> LogicalPixelsPerMm {
        self.base_px_per_mm
    }

    /// The explicit tracking-speed multiplier.
    #[must_use]
    pub const fn tracking_speed(&self) -> f64 {
        self.tracking_speed
    }
}

/// A signed two-dimensional millimeter displacement entering the stage.
///
/// Values must be finite (the Arbiter produces them from finite
/// [`crate::Millimeters`]); `process` still checks them fail-closed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FidelityDeltaMm {
    x: f64,
    y: f64,
}

impl FidelityDeltaMm {
    /// Creates a signed delta.
    #[must_use]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// The x displacement in millimeters.
    #[must_use]
    pub const fn x(self) -> f64 {
        self.x
    }

    /// The y displacement in millimeters.
    #[must_use]
    pub const fn y(self) -> f64 {
        self.y
    }
}

/// Runtime state of the fidelity stage (M11_TASK.md §6).
///
/// The Arbiter stores one `FidelityState` in its [`crate::ArbiterState`]
/// frame draft, so fidelity arithmetic commits atomically with the rest of
/// the state and a rejected frame rolls every field back.
///
/// The pixel remainder is deliberately **not** duplicated here: the Arbiter
/// keeps its existing `remainder_x_px`/`remainder_y_px` as the only pixel
/// remainder.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FidelityState {
    /// Timestamp of the last valid velocity sample (anchor). `None` before
    /// the first fidelity call.
    last_sample_timestamp: Option<Monotonic>,
    /// `P`: signed millimeters waiting for dead-zone release.
    pending_dead_zone_x_mm: f64,
    /// `P`: signed millimeters waiting for dead-zone release.
    pending_dead_zone_y_mm: f64,
    /// `V_pending`: signed millimeters waiting for a valid velocity sample.
    pending_velocity_x_mm: f64,
    /// `V_pending`: signed millimeters waiting for a valid velocity sample.
    pending_velocity_y_mm: f64,
    /// `t_acc`: positive elapsed seconds paired with `V_pending`.
    pending_velocity_seconds: f64,
    /// The filtered scalar velocity in millimeters per second.
    filtered_velocity_mm_per_s: f64,
}

impl FidelityState {
    /// Fresh, uninitialized fidelity state (no anchor, zero accumulators,
    /// zero filtered velocity).
    #[must_use]
    pub const fn fresh() -> Self {
        Self {
            last_sample_timestamp: None,
            pending_dead_zone_x_mm: 0.0,
            pending_dead_zone_y_mm: 0.0,
            pending_velocity_x_mm: 0.0,
            pending_velocity_y_mm: 0.0,
            pending_velocity_seconds: 0.0,
            filtered_velocity_mm_per_s: 0.0,
        }
    }

    /// The anchor timestamp of the last valid velocity sample.
    #[must_use]
    pub const fn last_sample_timestamp(&self) -> Option<Monotonic> {
        self.last_sample_timestamp
    }

    /// `P`: the signed dead-zone displacement in millimeters.
    #[must_use]
    pub const fn pending_dead_zone_mm(&self) -> (f64, f64) {
        (self.pending_dead_zone_x_mm, self.pending_dead_zone_y_mm)
    }

    /// `V_pending`: the signed velocity-sample displacement in millimeters.
    #[must_use]
    pub const fn pending_velocity_mm(&self) -> (f64, f64) {
        (self.pending_velocity_x_mm, self.pending_velocity_y_mm)
    }

    /// `t_acc`: the positive elapsed seconds paired with `V_pending`.
    #[must_use]
    pub const fn pending_velocity_seconds(&self) -> f64 {
        self.pending_velocity_seconds
    }

    /// The filtered scalar velocity in millimeters per second.
    #[must_use]
    pub const fn filtered_velocity_mm_per_s(&self) -> f64 {
        self.filtered_velocity_mm_per_s
    }

    /// Resets the timing accumulators (`V_pending`, `t_acc`, and the filtered
    /// velocity) but preserves the anchor. Used by a long-gap re-anchor,
    /// which discards `P` as well via [`FidelityState::reset_timing_and_p`].
    fn reset_timing(&mut self) {
        self.pending_velocity_x_mm = 0.0;
        self.pending_velocity_y_mm = 0.0;
        self.pending_velocity_seconds = 0.0;
        self.filtered_velocity_mm_per_s = 0.0;
    }

    /// Long-gap re-anchor: discard `P`, `V_pending`, `t_acc`, and the
    /// filtered velocity (the pixel remainder is preserved by the Arbiter).
    fn reset_timing_and_p(&mut self) {
        self.pending_dead_zone_x_mm = 0.0;
        self.pending_dead_zone_y_mm = 0.0;
        self.reset_timing();
    }
}

/// The result of one fidelity stage call.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FidelityOutcome {
    /// The displacement was retained (below the dead-zone radius, or a
    /// duplicate timestamp with no dead-zone evaluation): emit nothing.
    Hold,
    /// Emit the scaled, finite pixel deltas (base scale × gain × tracking
    /// speed applied; the prior pixel remainder is applied by the Arbiter).
    EmitScaledPixels { x: f64, y: f64 },
    /// A long gap re-anchored the stage and discarded the gap-crossing
    /// displacement: emit nothing (a normal outcome, not an error).
    Reanchored,
}

/// Runtime fidelity arithmetic failure: non-finite or overflowing.
///
/// Dead-zone hold, reset, and re-anchor are normal outcomes, not errors.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum FidelityError {
    /// Fidelity arithmetic produced a non-finite (`NaN`) value.
    #[error("fidelity arithmetic produced a non-finite value")]
    NonFinite,
    /// Fidelity arithmetic overflowed to infinity.
    #[error("fidelity arithmetic overflowed")]
    Overflow,
}

/// Returns `value` unchanged when it is finite; fails closed otherwise.
fn checked(value: f64) -> Result<f64, FidelityError> {
    if value.is_nan() {
        return Err(FidelityError::NonFinite);
    }
    if !value.is_finite() {
        return Err(FidelityError::Overflow);
    }
    Ok(value)
}

/// The smoothstep easing `w = t²(3 − 2t)` on `t ∈ [0, 1]`.
///
/// Continuous, monotonic non-decreasing on `[0, 1]`, with `w(0) = 0` and
/// `w(1) = 1`. The Arbiter clamps `t` before calling it, but the function
/// itself is total and finite for any finite `t`.
#[must_use]
pub fn smoothstep(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The gain for a filtered velocity: a continuous, monotonic non-decreasing
/// smoothstep between `min_gain` and `max_gain`, clamped to the configured
/// velocity range.
#[must_use]
pub fn gain(config: &FidelityConfig, velocity_mm_per_s: f64) -> f64 {
    let t = (velocity_mm_per_s - config.gain_x0_mm_per_s())
        / (config.gain_x1_mm_per_s() - config.gain_x0_mm_per_s());
    config.min_gain() + (config.max_gain() - config.min_gain()) * smoothstep(t)
}

/// The isotropic scalar `base_px_per_mm * gain * tracking_speed` for a
/// filtered velocity.
#[must_use]
pub fn scalar(config: &FidelityConfig, velocity_mm_per_s: f64) -> f64 {
    config.base_px_per_mm().as_px_per_mm() as f64
        * gain(config, velocity_mm_per_s)
        * config.tracking_speed()
}

/// Processes one committed one-finger pointer displacement.
///
/// `delta` is the signed millimeter displacement of the current committed
/// step; `timestamp` is the frame's monotonic timestamp. Arbiter's existing
/// sequence and timestamp regression checks run before this stage, so
/// timestamps are non-decreasing; defensively, a backwards `duration_since`
/// is treated as zero elapsed (never a fabricated negative time or velocity
/// sample).
///
/// # Errors
///
/// Returns [`FidelityError`] when fidelity arithmetic becomes non-finite or
/// overflows. On error the state may be partially mutated (the Arbiter
/// discards the whole frame draft, rolling the fidelity state back with it).
pub fn process(
    config: &FidelityConfig,
    state: &mut FidelityState,
    delta: FidelityDeltaMm,
    timestamp: Monotonic,
) -> Result<FidelityOutcome, FidelityError> {
    // Fail-closed input check: the delta must be finite.
    checked(delta.x())?;
    checked(delta.y())?;

    // First fidelity call: anchor, fold the whole displacement into P, keep
    // the velocity accumulators empty, and evaluate the dead zone at the
    // initial filtered velocity 0 (hence min_gain).
    let Some(last) = state.last_sample_timestamp else {
        state.last_sample_timestamp = Some(timestamp);
        state.pending_dead_zone_x_mm = checked(state.pending_dead_zone_x_mm + delta.x())?;
        state.pending_dead_zone_y_mm = checked(state.pending_dead_zone_y_mm + delta.y())?;
        return evaluate_dead_zone(config, state);
    };

    // Checked elapsed time. A backwards timestamp yields None (checked
    // monotonic arithmetic); treat it as zero elapsed (duplicate) rather than
    // fabricating a negative duration.
    let elapsed = timestamp.duration_since(last);
    let dt = elapsed.unwrap_or(Duration::ZERO);

    // Inclusive long gap — check BEFORE folding the gap-crossing delta.
    if dt >= config.long_gap() {
        state.reset_timing_and_p();
        state.last_sample_timestamp = Some(timestamp);
        return Ok(FidelityOutcome::Reanchored);
    }

    // Fold the frame delta into both P and V_pending.
    state.pending_dead_zone_x_mm = checked(state.pending_dead_zone_x_mm + delta.x())?;
    state.pending_dead_zone_y_mm = checked(state.pending_dead_zone_y_mm + delta.y())?;
    state.pending_velocity_x_mm = checked(state.pending_velocity_x_mm + delta.x())?;
    state.pending_velocity_y_mm = checked(state.pending_velocity_y_mm + delta.y())?;

    // Duplicate timestamp: add zero to t_acc, do not divide or update the
    // filtered velocity, and do NOT evaluate the dead zone — P merely
    // accumulates (M11_TASK.md §7.2). The displacement participates exactly
    // once in the next valid velocity sample.
    if dt.is_zero() {
        return Ok(FidelityOutcome::Hold);
    }

    // Positive elapsed time below the long gap: velocity update.
    let dt_secs = dt.as_secs_f64();
    state.pending_velocity_seconds = checked(state.pending_velocity_seconds + dt_secs)?;
    let speed = state
        .pending_velocity_x_mm
        .hypot(state.pending_velocity_y_mm)
        / state.pending_velocity_seconds;
    let speed = checked(speed)?;
    let tau_secs = config.velocity_tau().as_secs_f64();
    let alpha = checked(1.0 - (-state.pending_velocity_seconds / tau_secs).exp())?;
    let v_new = checked(alpha * speed + (1.0 - alpha) * state.filtered_velocity_mm_per_s)?;
    state.filtered_velocity_mm_per_s = v_new;
    state.pending_velocity_x_mm = 0.0;
    state.pending_velocity_y_mm = 0.0;
    state.pending_velocity_seconds = 0.0;
    state.last_sample_timestamp = Some(timestamp);

    evaluate_dead_zone(config, state)
}

/// Evaluates the signed radial dead zone on `P`: when the norm reaches the
/// configured radius (equality releases), scale and emit all of `P`, then
/// clear it; otherwise emit nothing and retain it.
fn evaluate_dead_zone(
    config: &FidelityConfig,
    state: &mut FidelityState,
) -> Result<FidelityOutcome, FidelityError> {
    let norm = state
        .pending_dead_zone_x_mm
        .hypot(state.pending_dead_zone_y_mm);
    if norm >= config.dead_zone_radius_mm() {
        let scalar = scalar(config, state.filtered_velocity_mm_per_s);
        let scaled_x = checked(state.pending_dead_zone_x_mm * scalar)?;
        let scaled_y = checked(state.pending_dead_zone_y_mm * scalar)?;
        state.pending_dead_zone_x_mm = 0.0;
        state.pending_dead_zone_y_mm = 0.0;
        Ok(FidelityOutcome::EmitScaledPixels {
            x: scaled_x,
            y: scaled_y,
        })
    } else {
        Ok(FidelityOutcome::Hold)
    }
}
