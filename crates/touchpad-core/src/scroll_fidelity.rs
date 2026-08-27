//! M12 two-finger scroll fidelity plus legacy kinetic-scroll primitives.
//!
//! The live Arbiter/takeover path uses this module for direct finger-scroll
//! filtering only. It now ends the scroll lifecycle when the contacts lift;
//! the momentum helpers remain available for settings compatibility and
//! reference tests but are not driven after finger release. Kinetic
//! continuation belongs at a higher layer that knows the scroll target.
//!
//! This module is pure and platform independent. It never reads a clock;
//! callers provide [`Monotonic`] timestamps from the same domain as
//! `ContactFrame.monotonic_timestamp`. It owns only physical scroll velocity,
//! axis-lock and momentum state. Pixel remainders remain owned by the Arbiter.

#![forbid(unsafe_code)]

use std::time::Duration;

use crate::time::Monotonic;
use crate::units::LogicalPixelsPerMm;

/// Active axis-lock state for M12 scroll fidelity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AxisLock {
    /// No axis is locked; diagonal scrolling remains two-dimensional.
    #[default]
    None,
    /// Horizontal intent dominates; vertical displacement is suppressed.
    Horizontal,
    /// Vertical intent dominates; horizontal displacement is suppressed.
    Vertical,
}

/// Validated M12 scroll-fidelity configuration.
#[derive(Clone, Debug, PartialEq)]
pub struct ScrollFidelityConfig {
    velocity_tau: Duration,
    gain_x0_mm_per_s: f64,
    gain_x1_mm_per_s: f64,
    min_gain: f64,
    max_gain: f64,
    axis_lock_engage_ratio: f64,
    axis_lock_release_ratio: f64,
    momentum_tau: Duration,
    momentum_start_speed_mm_per_s: f64,
    momentum_stop_speed_mm_per_s: f64,
    momentum_tick_cap: Duration,
}

impl ScrollFidelityConfig {
    /// Builds a validated M12 scroll-fidelity configuration.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        velocity_tau: Duration,
        gain_x0_mm_per_s: f64,
        gain_x1_mm_per_s: f64,
        min_gain: f64,
        max_gain: f64,
        axis_lock_engage_ratio: f64,
        axis_lock_release_ratio: f64,
        momentum_tau: Duration,
        momentum_start_speed_mm_per_s: f64,
        momentum_stop_speed_mm_per_s: f64,
        momentum_tick_cap: Duration,
    ) -> Result<Self, ScrollFidelityConfigError> {
        if velocity_tau.is_zero() {
            return Err(ScrollFidelityConfigError::ZeroDuration("velocity_tau"));
        }
        if momentum_tau.is_zero() {
            return Err(ScrollFidelityConfigError::ZeroDuration("momentum_tau"));
        }
        if momentum_tick_cap.is_zero() {
            return Err(ScrollFidelityConfigError::ZeroDuration("momentum_tick_cap"));
        }
        validate_positive_finite("gain_x0_mm_per_s", gain_x0_mm_per_s)?;
        validate_positive_finite("gain_x1_mm_per_s", gain_x1_mm_per_s)?;
        if gain_x1_mm_per_s <= gain_x0_mm_per_s {
            return Err(ScrollFidelityConfigError::InvalidOrdering(
                "gain_x1_mm_per_s must be greater than gain_x0_mm_per_s",
            ));
        }
        validate_positive_finite("min_gain", min_gain)?;
        validate_positive_finite("max_gain", max_gain)?;
        if max_gain < min_gain {
            return Err(ScrollFidelityConfigError::InvalidOrdering(
                "max_gain must be greater than or equal to min_gain",
            ));
        }
        validate_positive_finite("axis_lock_engage_ratio", axis_lock_engage_ratio)?;
        validate_positive_finite("axis_lock_release_ratio", axis_lock_release_ratio)?;
        if axis_lock_engage_ratio <= 1.0 {
            return Err(ScrollFidelityConfigError::InvalidOrdering(
                "axis_lock_engage_ratio must be greater than 1",
            ));
        }
        if axis_lock_release_ratio <= 1.0 || axis_lock_release_ratio >= axis_lock_engage_ratio {
            return Err(ScrollFidelityConfigError::InvalidOrdering(
                "axis_lock_release_ratio must be > 1 and < axis_lock_engage_ratio",
            ));
        }
        validate_positive_finite(
            "momentum_start_speed_mm_per_s",
            momentum_start_speed_mm_per_s,
        )?;
        validate_positive_finite("momentum_stop_speed_mm_per_s", momentum_stop_speed_mm_per_s)?;
        if momentum_stop_speed_mm_per_s >= momentum_start_speed_mm_per_s {
            return Err(ScrollFidelityConfigError::InvalidOrdering(
                "momentum_stop_speed_mm_per_s must be below momentum_start_speed_mm_per_s",
            ));
        }

        Ok(Self {
            velocity_tau,
            gain_x0_mm_per_s,
            gain_x1_mm_per_s,
            min_gain,
            max_gain,
            axis_lock_engage_ratio,
            axis_lock_release_ratio,
            momentum_tau,
            momentum_start_speed_mm_per_s,
            momentum_stop_speed_mm_per_s,
            momentum_tick_cap,
        })
    }

    #[must_use]
    pub const fn velocity_tau(&self) -> Duration {
        self.velocity_tau
    }

    #[must_use]
    pub const fn gain_x0_mm_per_s(&self) -> f64 {
        self.gain_x0_mm_per_s
    }

    #[must_use]
    pub const fn gain_x1_mm_per_s(&self) -> f64 {
        self.gain_x1_mm_per_s
    }

    #[must_use]
    pub const fn min_gain(&self) -> f64 {
        self.min_gain
    }

    #[must_use]
    pub const fn max_gain(&self) -> f64 {
        self.max_gain
    }

    #[must_use]
    pub const fn axis_lock_engage_ratio(&self) -> f64 {
        self.axis_lock_engage_ratio
    }

    #[must_use]
    pub const fn axis_lock_release_ratio(&self) -> f64 {
        self.axis_lock_release_ratio
    }

    #[must_use]
    pub const fn momentum_tau(&self) -> Duration {
        self.momentum_tau
    }

    #[must_use]
    pub const fn momentum_start_speed_mm_per_s(&self) -> f64 {
        self.momentum_start_speed_mm_per_s
    }

    #[must_use]
    pub const fn momentum_stop_speed_mm_per_s(&self) -> f64 {
        self.momentum_stop_speed_mm_per_s
    }

    #[must_use]
    pub const fn momentum_tick_cap(&self) -> Duration {
        self.momentum_tick_cap
    }
}

/// Construction failures for [`ScrollFidelityConfig`].
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum ScrollFidelityConfigError {
    #[error("M12 scroll fidelity requires a non-zero {0}")]
    ZeroDuration(&'static str),
    #[error("M12 scroll fidelity requires finite positive {name}, got {value}")]
    NonPositiveOrNonFinite { name: &'static str, value: f64 },
    #[error("invalid M12 scroll-fidelity ordering: {0}")]
    InvalidOrdering(&'static str),
}

fn validate_positive_finite(
    name: &'static str,
    value: f64,
) -> Result<(), ScrollFidelityConfigError> {
    if !value.is_finite() || value <= 0.0 {
        Err(ScrollFidelityConfigError::NonPositiveOrNonFinite { name, value })
    } else {
        Ok(())
    }
}

/// Runtime arithmetic errors from the pure M12 stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ScrollFidelityError {
    #[error("M12 scroll timestamp regressed")]
    TimestampRegression,
    #[error("M12 scroll arithmetic became non-finite")]
    NonFinite,
}

/// One direct two-finger scroll sample outcome before integer quantization.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScrollFidelityOutcome {
    /// No scaled delta is ready (currently only duplicate-timestamp folding).
    Hold,
    /// A scaled logical-pixel delta is ready for the Arbiter's existing
    /// per-axis remainder/quantization path.
    EmitScaledPixels {
        x: f64,
        y: f64,
        gain: f64,
        filtered_speed_mm_per_s: f64,
        axis_lock: AxisLock,
    },
}

/// One momentum tick outcome before integer quantization.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MomentumOutcome {
    Inactive,
    Hold,
    EmitScaledPixels { x: f64, y: f64 },
    End,
    EmitAndEnd { x: f64, y: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct MomentumState {
    last_tick: Monotonic,
    velocity_x_mm_per_s: f64,
    velocity_y_mm_per_s: f64,
    axis_lock: AxisLock,
}

/// Mutable state for the M12 pure scroll-fidelity stage.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollFidelityState {
    last_sample_timestamp: Option<Monotonic>,
    pending_x_mm: f64,
    pending_y_mm: f64,
    pending_time_secs: f64,
    filtered_velocity_x_mm_per_s: f64,
    filtered_velocity_y_mm_per_s: f64,
    axis_lock: AxisLock,
    momentum: Option<MomentumState>,
}

impl Default for ScrollFidelityState {
    fn default() -> Self {
        Self {
            last_sample_timestamp: None,
            pending_x_mm: 0.0,
            pending_y_mm: 0.0,
            pending_time_secs: 0.0,
            filtered_velocity_x_mm_per_s: 0.0,
            filtered_velocity_y_mm_per_s: 0.0,
            axis_lock: AxisLock::None,
            momentum: None,
        }
    }
}

impl ScrollFidelityState {
    #[must_use]
    pub const fn axis_lock(&self) -> AxisLock {
        self.axis_lock
    }

    #[must_use]
    pub fn filtered_velocity_mm_per_s(&self) -> (f64, f64) {
        (
            self.filtered_velocity_x_mm_per_s,
            self.filtered_velocity_y_mm_per_s,
        )
    }

    #[must_use]
    pub fn filtered_speed_mm_per_s(&self) -> f64 {
        hypot(
            self.filtered_velocity_x_mm_per_s,
            self.filtered_velocity_y_mm_per_s,
        )
    }

    #[must_use]
    pub const fn momentum_active(&self) -> bool {
        self.momentum.is_some()
    }

    /// Clears direct-sample and momentum state for a true lifecycle reset.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Cancels only software momentum. Direct sample history is retained;
    /// callers that start a new contact should use [`Self::reset`].
    pub fn cancel_momentum(&mut self) {
        self.momentum = None;
    }
}

/// Pure direct-scroll processing. `delta_mm` is the committed centroid delta
/// before natural-direction mapping. `base_px_per_mm` is the inherited M9
/// scroll scale. The first sample emits the full displacement at `min_gain`
/// without inventing velocity. Duplicate timestamps accumulate and defer
/// output until the next positive-dt sample.
pub fn process_scroll(
    config: &ScrollFidelityConfig,
    state: &mut ScrollFidelityState,
    delta_mm: (f64, f64),
    timestamp: Monotonic,
    base_px_per_mm: LogicalPixelsPerMm,
    natural: bool,
) -> Result<ScrollFidelityOutcome, ScrollFidelityError> {
    finite_pair(delta_mm)?;
    let base = f64::from(base_px_per_mm.as_px_per_mm());
    if !base.is_finite() || base <= 0.0 {
        return Err(ScrollFidelityError::NonFinite);
    }

    let Some(last) = state.last_sample_timestamp else {
        state.last_sample_timestamp = Some(timestamp);
        state.pending_x_mm = 0.0;
        state.pending_y_mm = 0.0;
        state.pending_time_secs = 0.0;
        let gain = config.min_gain;
        let sign = if natural { 1.0 } else { -1.0 };
        let x = delta_mm.0 * base * gain * sign;
        let y = delta_mm.1 * base * gain * sign;
        finite_pair((x, y))?;
        return Ok(ScrollFidelityOutcome::EmitScaledPixels {
            x,
            y,
            gain,
            filtered_speed_mm_per_s: 0.0,
            axis_lock: AxisLock::None,
        });
    };

    let elapsed = timestamp
        .duration_since(last)
        .ok_or(ScrollFidelityError::TimestampRegression)?;
    state.pending_x_mm += delta_mm.0;
    state.pending_y_mm += delta_mm.1;
    finite_pair((state.pending_x_mm, state.pending_y_mm))?;

    if elapsed.is_zero() {
        return Ok(ScrollFidelityOutcome::Hold);
    }

    let dt = elapsed.as_secs_f64();
    if !dt.is_finite() || dt <= 0.0 {
        return Err(ScrollFidelityError::NonFinite);
    }
    state.pending_time_secs += dt;
    if !state.pending_time_secs.is_finite() || state.pending_time_secs <= 0.0 {
        return Err(ScrollFidelityError::NonFinite);
    }

    let sample_vx = state.pending_x_mm / state.pending_time_secs;
    let sample_vy = state.pending_y_mm / state.pending_time_secs;
    finite_pair((sample_vx, sample_vy))?;

    let previous_v = (
        state.filtered_velocity_x_mm_per_s,
        state.filtered_velocity_y_mm_per_s,
    );
    let reversed =
        dot(previous_v, (sample_vx, sample_vy)) < 0.0 && hypot(previous_v.0, previous_v.1) > 0.0;

    let tau = config.velocity_tau.as_secs_f64();
    let alpha = 1.0 - (-state.pending_time_secs / tau).exp();
    if !alpha.is_finite() {
        return Err(ScrollFidelityError::NonFinite);
    }
    if reversed {
        state.filtered_velocity_x_mm_per_s = sample_vx;
        state.filtered_velocity_y_mm_per_s = sample_vy;
        state.axis_lock = AxisLock::None;
    } else {
        state.filtered_velocity_x_mm_per_s = alpha * sample_vx + (1.0 - alpha) * previous_v.0;
        state.filtered_velocity_y_mm_per_s = alpha * sample_vy + (1.0 - alpha) * previous_v.1;
    }
    finite_pair((
        state.filtered_velocity_x_mm_per_s,
        state.filtered_velocity_y_mm_per_s,
    ))?;

    state.axis_lock = update_axis_lock(
        config,
        state.axis_lock,
        state.filtered_velocity_x_mm_per_s,
        state.filtered_velocity_y_mm_per_s,
    );

    let raw_delta = (state.pending_x_mm, state.pending_y_mm);
    state.pending_x_mm = 0.0;
    state.pending_y_mm = 0.0;
    state.pending_time_secs = 0.0;
    state.last_sample_timestamp = Some(timestamp);

    let locked_delta = apply_axis_lock(state.axis_lock, raw_delta);
    let speed = state.filtered_speed_mm_per_s();
    let gain = gain_for_speed(config, speed)?;
    let sign = if natural { 1.0 } else { -1.0 };
    let x = locked_delta.0 * base * gain * sign;
    let y = locked_delta.1 * base * gain * sign;
    finite_pair((x, y))?;
    Ok(ScrollFidelityOutcome::EmitScaledPixels {
        x,
        y,
        gain,
        filtered_speed_mm_per_s: speed,
        axis_lock: state.axis_lock,
    })
}

/// Begins software momentum from the current filtered physical velocity.
/// Returns `true` when the start threshold is met. The caller keeps the
/// existing scroll lifecycle open only when this returns `true`.
pub fn begin_momentum(
    config: &ScrollFidelityConfig,
    state: &mut ScrollFidelityState,
    timestamp: Monotonic,
) -> Result<bool, ScrollFidelityError> {
    let speed = state.filtered_speed_mm_per_s();
    if !speed.is_finite() {
        return Err(ScrollFidelityError::NonFinite);
    }
    if speed < config.momentum_start_speed_mm_per_s {
        state.momentum = None;
        return Ok(false);
    }
    state.momentum = Some(MomentumState {
        last_tick: timestamp,
        velocity_x_mm_per_s: state.filtered_velocity_x_mm_per_s,
        velocity_y_mm_per_s: state.filtered_velocity_y_mm_per_s,
        axis_lock: state.axis_lock,
    });
    Ok(true)
}

/// Advances active software momentum to `timestamp`, returning scaled pixel
/// displacement before the Arbiter's existing remainder/quantization. Large
/// elapsed intervals are integrated in at most `momentum_tick_cap` chunks so
/// the velocity-dependent gain changes smoothly while the decay slows.
pub fn tick_momentum(
    config: &ScrollFidelityConfig,
    state: &mut ScrollFidelityState,
    timestamp: Monotonic,
    base_px_per_mm: LogicalPixelsPerMm,
    natural: bool,
) -> Result<MomentumOutcome, ScrollFidelityError> {
    let Some(mut momentum) = state.momentum else {
        return Ok(MomentumOutcome::Inactive);
    };
    let elapsed = timestamp
        .duration_since(momentum.last_tick)
        .ok_or(ScrollFidelityError::TimestampRegression)?;
    if elapsed.is_zero() {
        return Ok(MomentumOutcome::Hold);
    }
    let mut remaining = elapsed.as_secs_f64();
    if !remaining.is_finite() || remaining <= 0.0 {
        return Err(ScrollFidelityError::NonFinite);
    }

    let tau = config.momentum_tau.as_secs_f64();
    let cap = config.momentum_tick_cap.as_secs_f64();
    let base = f64::from(base_px_per_mm.as_px_per_mm());
    let sign = if natural { 1.0 } else { -1.0 };
    let mut total_px = (0.0, 0.0);
    let mut ended = false;

    while remaining > 0.0 {
        let speed = hypot(momentum.velocity_x_mm_per_s, momentum.velocity_y_mm_per_s);
        if !speed.is_finite() {
            return Err(ScrollFidelityError::NonFinite);
        }
        if speed <= config.momentum_stop_speed_mm_per_s {
            ended = true;
            break;
        }

        let dt = remaining.min(cap);
        let decay = (-dt / tau).exp();
        if !decay.is_finite() {
            return Err(ScrollFidelityError::NonFinite);
        }
        // Integral of v0 * exp(-t/tau) over this capped interval.
        let integral_factor = tau * (1.0 - decay);
        let mut displacement = (
            momentum.velocity_x_mm_per_s * integral_factor,
            momentum.velocity_y_mm_per_s * integral_factor,
        );
        displacement = apply_axis_lock(momentum.axis_lock, displacement);
        let gain = gain_for_speed(config, speed)?;
        total_px.0 += displacement.0 * base * gain * sign;
        total_px.1 += displacement.1 * base * gain * sign;
        finite_pair(total_px)?;

        momentum.velocity_x_mm_per_s *= decay;
        momentum.velocity_y_mm_per_s *= decay;
        finite_pair((momentum.velocity_x_mm_per_s, momentum.velocity_y_mm_per_s))?;
        remaining -= dt;
    }

    momentum.last_tick = timestamp;
    let final_speed = hypot(momentum.velocity_x_mm_per_s, momentum.velocity_y_mm_per_s);
    if final_speed <= config.momentum_stop_speed_mm_per_s {
        ended = true;
    }

    if ended {
        state.momentum = None;
        if total_px.0 == 0.0 && total_px.1 == 0.0 {
            Ok(MomentumOutcome::End)
        } else {
            Ok(MomentumOutcome::EmitAndEnd {
                x: total_px.0,
                y: total_px.1,
            })
        }
    } else {
        state.momentum = Some(momentum);
        if total_px.0 == 0.0 && total_px.1 == 0.0 {
            Ok(MomentumOutcome::Hold)
        } else {
            Ok(MomentumOutcome::EmitScaledPixels {
                x: total_px.0,
                y: total_px.1,
            })
        }
    }
}

/// Smoothstep gain used by both direct scrolling and momentum ticks.
pub fn gain_for_speed(
    config: &ScrollFidelityConfig,
    speed_mm_per_s: f64,
) -> Result<f64, ScrollFidelityError> {
    if !speed_mm_per_s.is_finite() || speed_mm_per_s < 0.0 {
        return Err(ScrollFidelityError::NonFinite);
    }
    let t = ((speed_mm_per_s - config.gain_x0_mm_per_s)
        / (config.gain_x1_mm_per_s - config.gain_x0_mm_per_s))
        .clamp(0.0, 1.0);
    let w = t * t * (3.0 - 2.0 * t);
    let gain = config.min_gain + (config.max_gain - config.min_gain) * w;
    if !gain.is_finite() {
        Err(ScrollFidelityError::NonFinite)
    } else {
        Ok(gain)
    }
}

fn update_axis_lock(
    config: &ScrollFidelityConfig,
    current: AxisLock,
    vx: f64,
    vy: f64,
) -> AxisLock {
    let ax = vx.abs();
    let ay = vy.abs();
    match current {
        AxisLock::None => {
            if ax > 0.0 && ax >= ay * config.axis_lock_engage_ratio {
                AxisLock::Horizontal
            } else if ay > 0.0 && ay >= ax * config.axis_lock_engage_ratio {
                AxisLock::Vertical
            } else {
                AxisLock::None
            }
        }
        AxisLock::Horizontal => {
            if ay > 0.0 && ax < ay * config.axis_lock_release_ratio {
                AxisLock::None
            } else {
                AxisLock::Horizontal
            }
        }
        AxisLock::Vertical => {
            if ax > 0.0 && ay < ax * config.axis_lock_release_ratio {
                AxisLock::None
            } else {
                AxisLock::Vertical
            }
        }
    }
}

fn apply_axis_lock(lock: AxisLock, delta: (f64, f64)) -> (f64, f64) {
    match lock {
        AxisLock::None => delta,
        AxisLock::Horizontal => (delta.0, 0.0),
        AxisLock::Vertical => (0.0, delta.1),
    }
}

fn hypot(x: f64, y: f64) -> f64 {
    x.hypot(y)
}

fn dot(a: (f64, f64), b: (f64, f64)) -> f64 {
    a.0 * b.0 + a.1 * b.1
}

fn finite_pair(pair: (f64, f64)) -> Result<(), ScrollFidelityError> {
    if pair.0.is_finite() && pair.1.is_finite() {
        Ok(())
    } else {
        Err(ScrollFidelityError::NonFinite)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> ScrollFidelityConfig {
        ScrollFidelityConfig::new(
            Duration::from_millis(30),
            25.0,
            450.0,
            1.0,
            1.75,
            2.5,
            1.5,
            Duration::from_millis(325),
            35.0,
            6.0,
            Duration::from_millis(16),
        )
        .unwrap()
    }

    fn scale() -> LogicalPixelsPerMm {
        LogicalPixelsPerMm::try_new(10.0).unwrap()
    }

    #[test]
    fn config_rejects_bad_boundaries() {
        let good = cfg();
        assert_eq!(good.axis_lock_engage_ratio(), 2.5);
        assert!(matches!(
            ScrollFidelityConfig::new(
                Duration::ZERO,
                25.0,
                450.0,
                1.0,
                1.75,
                2.5,
                1.5,
                Duration::from_millis(325),
                35.0,
                6.0,
                Duration::from_millis(16),
            ),
            Err(ScrollFidelityConfigError::ZeroDuration("velocity_tau"))
        ));
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(ScrollFidelityConfig::new(
                Duration::from_millis(30),
                bad,
                450.0,
                1.0,
                1.75,
                2.5,
                1.5,
                Duration::from_millis(325),
                35.0,
                6.0,
                Duration::from_millis(16),
            )
            .is_err());
        }
        assert!(ScrollFidelityConfig::new(
            Duration::from_millis(30),
            25.0,
            450.0,
            1.0,
            1.75,
            1.0,
            1.0,
            Duration::from_millis(325),
            35.0,
            6.0,
            Duration::from_millis(16),
        )
        .is_err());
        assert!(ScrollFidelityConfig::new(
            Duration::from_millis(30),
            25.0,
            450.0,
            1.0,
            1.75,
            2.5,
            1.5,
            Duration::from_millis(325),
            6.0,
            6.0,
            Duration::from_millis(16),
        )
        .is_err());
    }

    #[test]
    fn first_sample_is_full_min_gain_and_no_velocity() {
        let mut state = ScrollFidelityState::default();
        let out = process_scroll(
            &cfg(),
            &mut state,
            (1.0, 0.5),
            Monotonic::from_nanos(1_000),
            scale(),
            true,
        )
        .unwrap();
        assert_eq!(
            out,
            ScrollFidelityOutcome::EmitScaledPixels {
                x: 10.0,
                y: 5.0,
                gain: 1.0,
                filtered_speed_mm_per_s: 0.0,
                axis_lock: AxisLock::None,
            }
        );
        assert_eq!(state.filtered_velocity_mm_per_s(), (0.0, 0.0));
    }

    #[test]
    fn duplicate_timestamp_folds_until_positive_dt() {
        let mut state = ScrollFidelityState::default();
        let t0 = Monotonic::from_nanos(1_000_000);
        let _ = process_scroll(&cfg(), &mut state, (0.1, 0.0), t0, scale(), true).unwrap();
        assert_eq!(
            process_scroll(&cfg(), &mut state, (0.2, 0.0), t0, scale(), true).unwrap(),
            ScrollFidelityOutcome::Hold
        );
        let out = process_scroll(
            &cfg(),
            &mut state,
            (0.1, 0.0),
            Monotonic::from_nanos(11_000_000),
            scale(),
            true,
        )
        .unwrap();
        match out {
            ScrollFidelityOutcome::EmitScaledPixels { x, .. } => assert!(x >= 3.0),
            ScrollFidelityOutcome::Hold => panic!("positive dt must flush pending motion"),
        }
    }

    #[test]
    fn axis_lock_engages_and_releases_with_hysteresis() {
        let mut state = ScrollFidelityState::default();
        let c = cfg();
        let _ = process_scroll(
            &c,
            &mut state,
            (0.1, 0.0),
            Monotonic::from_nanos(0),
            scale(),
            true,
        )
        .unwrap();
        let out = process_scroll(
            &c,
            &mut state,
            (1.0, 0.05),
            Monotonic::from_nanos(10_000_000),
            scale(),
            true,
        )
        .unwrap();
        assert!(matches!(
            out,
            ScrollFidelityOutcome::EmitScaledPixels {
                axis_lock: AxisLock::Horizontal,
                y: 0.0,
                ..
            }
        ));
        let _ = process_scroll(
            &c,
            &mut state,
            (0.1, 1.0),
            Monotonic::from_nanos(20_000_000),
            scale(),
            true,
        )
        .unwrap();
        assert_eq!(state.axis_lock(), AxisLock::None);
    }

    #[test]
    fn reversal_resets_stale_lock_and_velocity_direction() {
        let mut state = ScrollFidelityState::default();
        let c = cfg();
        let _ = process_scroll(&c, &mut state, (0.1, 0.0), Monotonic::ZERO, scale(), true).unwrap();
        let _ = process_scroll(
            &c,
            &mut state,
            (1.0, 0.0),
            Monotonic::from_nanos(10_000_000),
            scale(),
            true,
        )
        .unwrap();
        assert_eq!(state.axis_lock(), AxisLock::Horizontal);
        let _ = process_scroll(
            &c,
            &mut state,
            (-1.0, 0.5),
            Monotonic::from_nanos(20_000_000),
            scale(),
            true,
        )
        .unwrap();
        assert_eq!(state.axis_lock(), AxisLock::None);
        assert!(state.filtered_velocity_mm_per_s().0 < 0.0);
    }

    #[test]
    fn gain_is_bounded_and_monotonic() {
        let c = cfg();
        let mut previous = 0.0;
        for speed in [0.0, 25.0, 50.0, 100.0, 200.0, 450.0, 900.0] {
            let gain = gain_for_speed(&c, speed).unwrap();
            assert!(gain >= c.min_gain() && gain <= c.max_gain());
            assert!(gain >= previous);
            previous = gain;
        }
    }

    #[test]
    fn momentum_starts_decays_and_ends() {
        let c = cfg();
        let mut state = ScrollFidelityState::default();
        let _ = process_scroll(&c, &mut state, (0.1, 0.0), Monotonic::ZERO, scale(), true).unwrap();
        for i in 1..=8 {
            let _ = process_scroll(
                &c,
                &mut state,
                (1.0, 0.0),
                Monotonic::from_nanos(i * 10_000_000),
                scale(),
                true,
            )
            .unwrap();
        }
        let start = Monotonic::from_nanos(80_000_000);
        assert!(begin_momentum(&c, &mut state, start).unwrap());
        assert!(state.momentum_active());
        let first = tick_momentum(
            &c,
            &mut state,
            Monotonic::from_nanos(96_000_000),
            scale(),
            true,
        )
        .unwrap();
        assert!(matches!(
            first,
            MomentumOutcome::EmitScaledPixels { x, y: 0.0 } if x > 0.0
        ));
        let mut ended = false;
        for i in 7..500 {
            let out = tick_momentum(
                &c,
                &mut state,
                Monotonic::from_nanos(i * 16_000_000),
                scale(),
                true,
            )
            .unwrap();
            if matches!(
                out,
                MomentumOutcome::End | MomentumOutcome::EmitAndEnd { .. }
            ) {
                ended = true;
                break;
            }
        }
        assert!(ended);
        assert!(!state.momentum_active());
    }

    #[test]
    fn natural_false_reverses_output_only_not_physical_velocity() {
        let mut state = ScrollFidelityState::default();
        let out = process_scroll(
            &cfg(),
            &mut state,
            (1.0, -0.5),
            Monotonic::ZERO,
            scale(),
            false,
        )
        .unwrap();
        assert!(matches!(
            out,
            ScrollFidelityOutcome::EmitScaledPixels {
                x: -10.0,
                y: 5.0,
                ..
            }
        ));
        assert_eq!(state.filtered_velocity_mm_per_s(), (0.0, 0.0));
    }
}
