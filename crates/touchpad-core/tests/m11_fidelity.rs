//! M11 public API and end-to-end Arbiter contract tests for the
//! experimental one-finger pointer-fidelity stage (`m11-fidelity-v1`,
//! M11_TASK.md).
//!
//! Coverage required by M11_TASK.md §12 / M11_EXECUTION_TASK.md §3–§4:
//!
//! * every `FidelityConfig` validation boundary (NaN/infinity/zero/negative
//!   where applicable, and valid `min_gain == max_gain`);
//! * first-call full-motion preservation at `min_gain` and exclusion from
//!   the next velocity numerator;
//! * first-call sub-radius hold;
//! * duplicate timestamps — including a duplicate frame that pushes `P` over
//!   the dead-zone radius but still must not flush, and repeated zero-`dt`
//!   frames that never fabricate velocity;
//! * long-gap at `long_gap - 1 ns`, exactly `long_gap`, and above it;
//! * signed cancellation, slow monotonic release, reversals and diagonals;
//! * smoothstep/gain continuity, monotonicity, and bounds;
//! * isotropic scaling and the tracking multiplier;
//! * runtime non-finite/overflow failure where constructible through public
//!   APIs;
//! * the same constant physical motion at 60 Hz and 120 Hz: relative
//!   difference in filtered velocity, gain, and scalar each `<= 1%`;
//! * `M11Profile` exact constants and inheritance of every exposed M10
//!   value; the M11 Arbiter config is exactly the M10 Arbiter config plus
//!   fidelity (no copied M7–M9 constants).
//!
//! All tests are offline and pure: no hardware, no portal/libei session, no
//! desktop emission, no sleeping, no system-state change.

use std::time::Duration;

use touchpad_core::{
    gain, process, scalar, smoothstep, Arbiter, ArbiterConfig, ArbiterError, Contact, ContactFrame,
    ContactState, FidelityConfig, FidelityConfigError, FidelityDeltaMm, FidelityError,
    FidelityOutcome, FidelityState, Lifecycle, LifecycleTransition, LogicalPixels,
    LogicalPixelsPerMm, M10Profile, M11Profile, Millimeters, Monotonic, MouseButton, OutputEvent,
    PhysicalButtons, TapDragPhase, TwoFingerPhase,
};

/// The exact M11 provisional fidelity configuration (M11_TASK.md §10).
fn base_config() -> FidelityConfig {
    FidelityConfig::new(
        0.09,
        Duration::from_millis(20),
        Duration::from_millis(150),
        50.0,
        600.0,
        1.0,
        2.0,
        LogicalPixelsPerMm::try_new(10.0).unwrap(),
        1.0,
    )
    .expect("documented M11 constants validate")
}

fn ts(nanos: u64) -> Monotonic {
    Monotonic::from_nanos(nanos)
}

/// The elapsed time between two timestamps in seconds.
fn dt_secs(from: u64, to: u64) -> f64 {
    ts(to).duration_since(ts(from)).unwrap().as_secs_f64()
}

// ---------------------------------------------------------------------------
// Configuration validation boundaries
// ---------------------------------------------------------------------------

#[test]
fn config_accepts_the_documented_values() {
    let config = base_config();
    assert_eq!(config.dead_zone_radius_mm(), 0.09);
    assert_eq!(config.velocity_tau(), Duration::from_millis(20));
    assert_eq!(config.long_gap(), Duration::from_millis(150));
    assert_eq!(config.gain_x0_mm_per_s(), 50.0);
    assert_eq!(config.gain_x1_mm_per_s(), 600.0);
    assert_eq!(config.min_gain(), 1.0);
    assert_eq!(config.max_gain(), 2.0);
    assert_eq!(
        config.base_px_per_mm(),
        LogicalPixelsPerMm::try_new(10.0).unwrap()
    );
    assert_eq!(config.tracking_speed(), 1.0);
}

#[test]
fn config_rejects_non_finite_or_non_positive_dead_zone_radius() {
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0.0, -0.1] {
        let result = FidelityConfig::new(
            bad,
            Duration::from_millis(20),
            Duration::from_millis(150),
            50.0,
            600.0,
            1.0,
            2.0,
            LogicalPixelsPerMm::try_new(10.0).unwrap(),
            1.0,
        );
        assert!(
            matches!(result, Err(FidelityConfigError::DeadZoneRadius(got)) if (bad.is_nan() && got.is_nan()) || got == bad),
            "radius {bad}: got {result:?}"
        );
    }
}

#[test]
fn config_rejects_zero_velocity_tau_and_long_gap() {
    assert_eq!(
        FidelityConfig::new(
            0.09,
            Duration::ZERO,
            Duration::from_millis(150),
            50.0,
            600.0,
            1.0,
            2.0,
            LogicalPixelsPerMm::try_new(10.0).unwrap(),
            1.0,
        ),
        Err(FidelityConfigError::ZeroVelocityTau)
    );
    assert_eq!(
        FidelityConfig::new(
            0.09,
            Duration::from_millis(20),
            Duration::ZERO,
            50.0,
            600.0,
            1.0,
            2.0,
            LogicalPixelsPerMm::try_new(10.0).unwrap(),
            1.0,
        ),
        Err(FidelityConfigError::ZeroLongGap)
    );
}

#[test]
fn config_rejects_non_finite_or_non_positive_gain_x0() {
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0.0, -1.0] {
        let result = FidelityConfig::new(
            0.09,
            Duration::from_millis(20),
            Duration::from_millis(150),
            bad,
            600.0,
            1.0,
            2.0,
            LogicalPixelsPerMm::try_new(10.0).unwrap(),
            1.0,
        );
        assert!(
            matches!(result, Err(FidelityConfigError::GainX0(got)) if (bad.is_nan() && got.is_nan()) || got == bad),
            "x0 {bad}: got {result:?}"
        );
    }
}

#[test]
fn config_rejects_non_finite_or_non_greater_gain_x1() {
    for bad in [
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        50.0, // equal to x0
        10.0, // below x0
    ] {
        let result = FidelityConfig::new(
            0.09,
            Duration::from_millis(20),
            Duration::from_millis(150),
            50.0,
            bad,
            1.0,
            2.0,
            LogicalPixelsPerMm::try_new(10.0).unwrap(),
            1.0,
        );
        assert!(
            matches!(result, Err(FidelityConfigError::GainX1(got)) if (bad.is_nan() && got.is_nan()) || got == bad),
            "x1 {bad}: got {result:?}"
        );
    }
}

#[test]
fn config_rejects_non_finite_or_non_positive_min_gain() {
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0.0, -0.5] {
        let result = FidelityConfig::new(
            0.09,
            Duration::from_millis(20),
            Duration::from_millis(150),
            50.0,
            600.0,
            bad,
            2.0,
            LogicalPixelsPerMm::try_new(10.0).unwrap(),
            1.0,
        );
        assert!(
            matches!(result, Err(FidelityConfigError::MinGain(got)) if (bad.is_nan() && got.is_nan()) || got == bad),
            "min_gain {bad}: got {result:?}"
        );
    }
}

#[test]
fn config_rejects_max_gain_below_min_gain_or_non_finite() {
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0.5, 0.999] {
        let result = FidelityConfig::new(
            0.09,
            Duration::from_millis(20),
            Duration::from_millis(150),
            50.0,
            600.0,
            1.0,
            bad,
            LogicalPixelsPerMm::try_new(10.0).unwrap(),
            1.0,
        );
        assert!(
            matches!(result, Err(FidelityConfigError::MaxGain(got)) if (bad.is_nan() && got.is_nan()) || got == bad),
            "max_gain {bad}: got {result:?}"
        );
    }
}

#[test]
fn config_accepts_min_gain_equal_to_max_gain() {
    // min_gain == max_gain is valid: the curve is a flat line at that gain.
    let config = FidelityConfig::new(
        0.09,
        Duration::from_millis(20),
        Duration::from_millis(150),
        50.0,
        600.0,
        1.5,
        1.5,
        LogicalPixelsPerMm::try_new(10.0).unwrap(),
        1.0,
    )
    .expect("min_gain == max_gain must validate");
    assert_eq!(config.min_gain(), 1.5);
    assert_eq!(config.max_gain(), 1.5);
    // The gain is the flat value everywhere.
    assert_eq!(gain(&config, 0.0), 1.5);
    assert_eq!(gain(&config, 300.0), 1.5);
    assert_eq!(gain(&config, 1000.0), 1.5);
}

#[test]
fn config_rejects_non_finite_or_non_positive_tracking_speed() {
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0.0, -2.0] {
        let result = FidelityConfig::new(
            0.09,
            Duration::from_millis(20),
            Duration::from_millis(150),
            50.0,
            600.0,
            1.0,
            2.0,
            LogicalPixelsPerMm::try_new(10.0).unwrap(),
            bad,
        );
        assert!(
            matches!(result, Err(FidelityConfigError::TrackingSpeed(got)) if (bad.is_nan() && got.is_nan()) || got == bad),
            "tracking_speed {bad}: got {result:?}"
        );
    }
}

#[test]
fn config_rejects_non_finite_base_scale_via_its_own_validated_type() {
    // The base scale is the existing validated type: NaN/infinity/zero/
    // negative are rejected by LogicalPixelsPerMm::try_new, before any
    // FidelityConfig construction is even possible.
    for bad in [f64::NAN, f64::INFINITY, 0.0, -1.0] {
        assert!(
            LogicalPixelsPerMm::try_new(bad as f32).is_err(),
            "base scale {bad} must be rejected by the existing type"
        );
    }
}

// ---------------------------------------------------------------------------
// Gain curve: smoothstep continuity, monotonicity, bounds
// ---------------------------------------------------------------------------

#[test]
fn smoothstep_is_a_monotone_ramp_between_zero_and_one() {
    assert_eq!(smoothstep(0.0), 0.0);
    assert_eq!(smoothstep(1.0), 1.0);
    assert_eq!(smoothstep(0.5), 0.5);
    // Monotonic non-decreasing across the unit interval.
    let mut previous = -1.0_f64;
    for i in 0..=1000 {
        let t = i as f64 / 1000.0;
        let w = smoothstep(t);
        assert!(w >= previous, "smoothstep must be non-decreasing at {t}");
        previous = w;
    }
    // Out-of-range inputs are clamped, staying total and finite.
    assert_eq!(smoothstep(-0.5), 0.0);
    assert_eq!(smoothstep(1.5), 1.0);
}

#[test]
fn gain_is_continuous_monotonic_bounded_and_finite() {
    let config = base_config();
    // At and below x0 the gain is min_gain; at and above x1 it is max_gain.
    assert_eq!(gain(&config, 0.0), config.min_gain());
    assert_eq!(gain(&config, 50.0), config.min_gain());
    assert_eq!(gain(&config, 600.0), config.max_gain());
    assert_eq!(gain(&config, 10_000.0), config.max_gain());
    // Monotonic non-decreasing over a wide velocity sweep.
    let mut previous = 0.0_f64;
    for i in 0..=2000 {
        let v = i as f64 * 0.5;
        let g = gain(&config, v);
        assert!(g >= previous, "gain must be non-decreasing at {v} mm/s");
        assert!(
            (config.min_gain()..=config.max_gain()).contains(&g),
            "gain {g} out of bounds at {v} mm/s"
        );
        assert!(g.is_finite(), "gain must be finite at {v} mm/s");
        previous = g;
    }
    // Continuity: the gain approaches the same value from below and above x0
    // and x1 (smoothstep is C1 at the clamp boundaries; here we check the
    // functional continuity, i.e. no jump).
    let below_x0 = gain(&config, config.gain_x0_mm_per_s() - 1e-9);
    let at_x0 = gain(&config, config.gain_x0_mm_per_s());
    let above_x0 = gain(&config, config.gain_x0_mm_per_s() + 1e-9);
    assert!((below_x0 - at_x0).abs() < 1e-12);
    assert!((above_x0 - at_x0).abs() < 1e-9);
    let below_x1 = gain(&config, config.gain_x1_mm_per_s() - 1e-9);
    let at_x1 = gain(&config, config.gain_x1_mm_per_s());
    let above_x1 = gain(&config, config.gain_x1_mm_per_s() + 1e-9);
    assert!((below_x1 - at_x1).abs() < 1e-9);
    assert!((above_x1 - at_x1).abs() < 1e-12);
}

#[test]
fn scalar_is_isotropic_and_includes_the_tracking_multiplier() {
    let config = base_config();
    // The scalar is a single number applied to both axes: the per-axis
    // output of the stage for the same physical delta is identical.
    let mut state = FidelityState::fresh();
    let outcome = process(
        &config,
        &mut state,
        FidelityDeltaMm::new(0.5, 0.5),
        ts(1_000_000),
    )
    .unwrap();
    match outcome {
        FidelityOutcome::EmitScaledPixels { x, y } => {
            assert_eq!(x, y, "isotropic scalar must scale both axes equally");
            assert!((x - 0.5 * scalar(&config, 0.0)).abs() < 1e-9);
        }
        other => panic!("expected an emission, got {other:?}"),
    }

    // Tracking speed scales the output linearly and isotropically.
    let fast = FidelityConfig::new(
        0.09,
        Duration::from_millis(20),
        Duration::from_millis(150),
        50.0,
        600.0,
        1.0,
        2.0,
        LogicalPixelsPerMm::try_new(10.0).unwrap(),
        2.5,
    )
    .unwrap();
    assert!((scalar(&fast, 0.0) - 2.5 * scalar(&config, 0.0)).abs() < 1e-12);
    assert!((scalar(&fast, 200.0) - 2.5 * scalar(&config, 200.0)).abs() < 1e-12);
}

// ---------------------------------------------------------------------------
// First fidelity call
// ---------------------------------------------------------------------------

#[test]
fn first_call_preserves_full_motion_at_min_gain_and_excludes_it_from_velocity() {
    let config = base_config();
    let mut state = FidelityState::fresh();

    // First call: the whole accepted displacement (the M7 candidate's
    // accumulated delta) is folded into P and released at the initial
    // filtered velocity 0 -> min_gain (1.0), so 5 mm * 10 px/mm = 50 px.
    let first = process(
        &config,
        &mut state,
        FidelityDeltaMm::new(5.0, 0.0),
        ts(1_000_000_000),
    )
    .unwrap();
    assert_eq!(first, FidelityOutcome::EmitScaledPixels { x: 50.0, y: 0.0 });

    // The first displacement must NOT enter the next velocity numerator:
    // the second sample's speed is 1 mm / 16 ms = 62.5 mm/s, not
    // (5 + 1) mm / 16 ms = 375 mm/s.
    let second = process(
        &config,
        &mut state,
        FidelityDeltaMm::new(1.0, 0.0),
        ts(1_016_000_000),
    )
    .unwrap();
    let expected_speed = 1.0 / dt_secs(1_000_000_000, 1_016_000_000);
    assert!((expected_speed - 62.5).abs() < 1e-9);
    let expected_alpha = 1.0 - (-(dt_secs(1_000_000_000, 1_016_000_000) / 0.02)).exp();
    let expected_v = expected_alpha * expected_speed;
    let actual_v = state.filtered_velocity_mm_per_s();
    assert!(
        (actual_v - expected_v).abs() < 1e-9,
        "velocity {actual_v} must be {expected_v} (first displacement excluded from the numerator)"
    );
    // The second frame also released 1 mm at that velocity.
    assert!(matches!(
        second,
        FidelityOutcome::EmitScaledPixels { x, y } if y == 0.0 && x > 0.0
    ));
    // The velocity accumulators are cleared after the update.
    assert_eq!(state.pending_velocity_mm(), (0.0, 0.0));
    assert_eq!(state.pending_velocity_seconds(), 0.0);
}

#[test]
fn first_call_sub_radius_delta_is_held_and_retained() {
    let config = base_config();
    let mut state = FidelityState::fresh();
    let outcome = process(
        &config,
        &mut state,
        FidelityDeltaMm::new(0.05, 0.0),
        ts(1_000_000_000),
    )
    .unwrap();
    assert_eq!(outcome, FidelityOutcome::Hold);
    // P retains the sub-radius displacement; nothing else was fabricated.
    assert_eq!(state.pending_dead_zone_mm(), (0.05, 0.0));
    assert_eq!(state.pending_velocity_mm(), (0.0, 0.0));
    assert_eq!(state.pending_velocity_seconds(), 0.0);
    assert_eq!(state.filtered_velocity_mm_per_s(), 0.0);
    assert_eq!(state.last_sample_timestamp(), Some(ts(1_000_000_000)));
}

#[test]
fn first_call_does_not_fabricate_elapsed_time_or_velocity() {
    let config = base_config();
    let mut state = FidelityState::fresh();
    let _ = process(
        &config,
        &mut state,
        FidelityDeltaMm::new(5.0, 0.0),
        ts(42_000_000_000),
    )
    .unwrap();
    // No elapsed time and no velocity sample: the pre-commit interval is
    // unknown and must not be invented.
    assert_eq!(state.pending_velocity_mm(), (0.0, 0.0));
    assert_eq!(state.pending_velocity_seconds(), 0.0);
    assert_eq!(state.filtered_velocity_mm_per_s(), 0.0);
}

// ---------------------------------------------------------------------------
// Duplicate timestamps (dt == 0)
// ---------------------------------------------------------------------------

#[test]
fn duplicate_frame_folds_into_p_and_v_pending_without_flushing() {
    let config = base_config();
    let mut state = FidelityState::fresh();
    // First call: 0.05 mm held (sub-radius).
    let first = process(
        &config,
        &mut state,
        FidelityDeltaMm::new(0.05, 0.0),
        ts(1_000_000_000),
    )
    .unwrap();
    assert_eq!(first, FidelityOutcome::Hold);

    // Duplicate frame: the delta pushes P to 0.10 mm >= 0.09 mm radius, but
    // the dead zone must NOT be evaluated/flushed on a zero-dt frame — P
    // merely accumulates (M11_TASK.md §7.2).
    let duplicate = process(
        &config,
        &mut state,
        FidelityDeltaMm::new(0.05, 0.0),
        ts(1_000_000_000),
    )
    .unwrap();
    assert_eq!(duplicate, FidelityOutcome::Hold);
    assert_eq!(
        state.pending_dead_zone_mm(),
        (0.10, 0.0),
        "P must accumulate over the radius on a duplicate frame without flushing"
    );
    assert_eq!(
        state.pending_velocity_mm(),
        (0.05, 0.0),
        "the duplicate displacement enters V_pending exactly once"
    );
    assert_eq!(state.pending_velocity_seconds(), 0.0);
    assert_eq!(
        state.filtered_velocity_mm_per_s(),
        0.0,
        "no velocity update on a zero-dt frame"
    );

    // The next positive-dt frame consumes the accumulated duplicate
    // displacement exactly once in its velocity sample, and the dead zone
    // now releases all of P (0.10 mm).
    let next = process(
        &config,
        &mut state,
        FidelityDeltaMm::new(0.0, 0.0),
        ts(1_010_000_000),
    )
    .unwrap();
    let dt = dt_secs(1_000_000_000, 1_010_000_000);
    let expected_speed = 0.05 / dt; // only the duplicate displacement
    let expected_v = (1.0 - (-(dt / 0.02)).exp()) * expected_speed;
    assert!((state.filtered_velocity_mm_per_s() - expected_v).abs() < 1e-9);
    assert!(matches!(
        next,
        FidelityOutcome::EmitScaledPixels { x, y } if y == 0.0 && (x - 0.10 * scalar(&config, expected_v)).abs() < 1e-9
    ));
    assert_eq!(state.pending_dead_zone_mm(), (0.0, 0.0));
    assert_eq!(state.pending_velocity_mm(), (0.0, 0.0));
}

#[test]
fn repeated_zero_dt_frames_never_fabricate_velocity() {
    let config = base_config();
    let mut state = FidelityState::fresh();
    // Anchor with a small held delta.
    let _ = process(
        &config,
        &mut state,
        FidelityDeltaMm::new(0.05, 0.0),
        ts(1_000_000_000),
    )
    .unwrap();
    // Many duplicate frames with real displacement: no division, no velocity.
    for _ in 0..50 {
        let outcome = process(
            &config,
            &mut state,
            FidelityDeltaMm::new(0.01, 0.0),
            ts(1_000_000_000),
        )
        .unwrap();
        assert_eq!(outcome, FidelityOutcome::Hold);
        assert_eq!(state.filtered_velocity_mm_per_s(), 0.0);
        assert_eq!(state.pending_velocity_seconds(), 0.0);
    }
    // The accumulated displacement is exact in the accumulators (all values
    // finite; approximate comparison avoids last-ulp float drift from the
    // repeated additions).
    let (px, _) = state.pending_dead_zone_mm();
    assert!((px - (0.05 + 50.0 * 0.01)).abs() < 1e-12, "P.x = {px}");
    let (vx, _) = state.pending_velocity_mm();
    assert!((vx - 50.0 * 0.01).abs() < 1e-12, "V_pending.x = {vx}");
}

#[test]
fn duplicate_timestamp_is_not_a_long_gap() {
    // A duplicate frame at the same timestamp as a previous sample must take
    // the dt == 0 path (fold, hold), never the long-gap re-anchor path —
    // even when the gap between two *different* timestamps would be huge.
    let config = base_config();
    let mut state = FidelityState::fresh();
    let _ = process(
        &config,
        &mut state,
        FidelityDeltaMm::new(0.05, 0.0),
        ts(1_000_000_000),
    )
    .unwrap();
    // A duplicate of a *much later* timestamp (same as the anchor).
    let outcome = process(
        &config,
        &mut state,
        FidelityDeltaMm::new(0.05, 0.0),
        ts(1_000_000_000),
    )
    .unwrap();
    assert_eq!(outcome, FidelityOutcome::Hold);
    assert_eq!(state.pending_dead_zone_mm(), (0.10, 0.0));
    assert_eq!(state.last_sample_timestamp(), Some(ts(1_000_000_000)));
}

// ---------------------------------------------------------------------------
// Positive elapsed time below the long gap
// ---------------------------------------------------------------------------

#[test]
fn positive_dt_updates_velocity_and_evaluates_the_dead_zone() {
    let config = base_config();
    let mut state = FidelityState::fresh();
    let _ = process(
        &config,
        &mut state,
        FidelityDeltaMm::new(1.0, 0.0),
        ts(1_000_000_000),
    )
    .unwrap();
    let outcome = process(
        &config,
        &mut state,
        FidelityDeltaMm::new(1.0, 0.0),
        ts(1_020_000_000),
    )
    .unwrap();
    let dt = dt_secs(1_000_000_000, 1_020_000_000);
    let expected_speed = 1.0 / dt;
    let expected_v = (1.0 - (-(dt / 0.02)).exp()) * expected_speed;
    assert!((state.filtered_velocity_mm_per_s() - expected_v).abs() < 1e-9);
    assert!(matches!(
        outcome,
        FidelityOutcome::EmitScaledPixels { x, y } if y == 0.0 && (x - 1.0 * scalar(&config, expected_v)).abs() < 1e-9
    ));
    assert_eq!(state.pending_velocity_mm(), (0.0, 0.0));
    assert_eq!(state.pending_velocity_seconds(), 0.0);
    assert_eq!(state.last_sample_timestamp(), Some(ts(1_020_000_000)));
}

#[test]
fn ema_approaches_the_physical_speed_after_several_samples() {
    // Constant 100 mm/s motion at 100 Hz: after enough samples the filtered
    // velocity must converge toward 100 mm/s (time-domain EMA).
    let config = base_config();
    let mut state = FidelityState::fresh();
    let dt = 10_000_000u64; // 10 ms
    let delta = 1.0; // 100 mm/s * 10 ms
    let _ = process(&config, &mut state, FidelityDeltaMm::new(delta, 0.0), ts(0)).unwrap();
    for i in 1..=200 {
        let _ = process(
            &config,
            &mut state,
            FidelityDeltaMm::new(delta, 0.0),
            ts(i * dt),
        )
        .unwrap();
    }
    let v = state.filtered_velocity_mm_per_s();
    assert!(
        (v - 100.0).abs() < 0.1,
        "filtered velocity {v} must converge toward 100 mm/s"
    );
}

// ---------------------------------------------------------------------------
// Long gap (dt >= long_gap, inclusive, checked before folding)
// ---------------------------------------------------------------------------

#[test]
fn long_gap_at_long_gap_minus_1_ns_is_a_normal_fold_and_update() {
    let config = base_config();
    let mut state = FidelityState::fresh();
    let long_gap_ns = config.long_gap().as_nanos() as u64;
    let _ = process(
        &config,
        &mut state,
        FidelityDeltaMm::new(1.0, 0.0),
        ts(1_000_000_000),
    )
    .unwrap();
    // dt = long_gap - 1 ns: still a normal positive-time sample.
    let outcome = process(
        &config,
        &mut state,
        FidelityDeltaMm::new(1.0, 0.0),
        ts(1_000_000_000 + long_gap_ns - 1),
    )
    .unwrap();
    assert!(
        matches!(outcome, FidelityOutcome::EmitScaledPixels { .. }),
        "long_gap - 1 ns must fold normally, got {outcome:?}"
    );
    assert_eq!(
        state.last_sample_timestamp(),
        Some(ts(1_000_000_000 + long_gap_ns - 1))
    );
    assert!(state.filtered_velocity_mm_per_s() > 0.0);
}

#[test]
fn long_gap_exactly_reanchors_and_discards_the_gap_crossing_delta() {
    let config = base_config();
    let mut state = FidelityState::fresh();
    let long_gap_ns = config.long_gap().as_nanos() as u64;
    // Build up state: an anchored first call with a held P.
    let _ = process(
        &config,
        &mut state,
        FidelityDeltaMm::new(0.05, 0.05),
        ts(1_000_000_000),
    )
    .unwrap();
    // A positive-dt sample first, so velocity and accumulators are live.
    let _ = process(
        &config,
        &mut state,
        FidelityDeltaMm::new(0.3, 0.1),
        ts(1_010_000_000),
    )
    .unwrap();
    assert!(state.filtered_velocity_mm_per_s() > 0.0);

    // dt == long_gap exactly: the boundary is inclusive; the gap-crossing
    // displacement is discarded, everything is cleared, the timestamp is
    // re-anchored, and Reanchored is a normal outcome (not an error).
    let outcome = process(
        &config,
        &mut state,
        FidelityDeltaMm::new(5.0, 5.0),
        ts(1_010_000_000 + long_gap_ns),
    )
    .unwrap();
    assert_eq!(outcome, FidelityOutcome::Reanchored);
    assert_eq!(state.pending_dead_zone_mm(), (0.0, 0.0));
    assert_eq!(state.pending_velocity_mm(), (0.0, 0.0));
    assert_eq!(state.pending_velocity_seconds(), 0.0);
    assert_eq!(state.filtered_velocity_mm_per_s(), 0.0);
    assert_eq!(
        state.last_sample_timestamp(),
        Some(ts(1_010_000_000 + long_gap_ns)),
        "the gap-crossing frame becomes the new anchor"
    );

    // A duplicate of the re-anchored timestamp is a normal duplicate, not a
    // long gap: the new displacement folds into P (discarded state stays
    // fresh until the next positive sample).
    let after = process(
        &config,
        &mut state,
        FidelityDeltaMm::new(0.05, 0.0),
        ts(1_010_000_000 + long_gap_ns),
    )
    .unwrap();
    assert_eq!(after, FidelityOutcome::Hold);
    assert_eq!(state.pending_dead_zone_mm(), (0.05, 0.0));
}

#[test]
fn long_gap_above_the_boundary_reanchors() {
    let config = base_config();
    let mut state = FidelityState::fresh();
    let long_gap_ns = config.long_gap().as_nanos() as u64;
    let _ = process(
        &config,
        &mut state,
        FidelityDeltaMm::new(0.05, 0.0),
        ts(1_000_000_000),
    )
    .unwrap();
    let outcome = process(
        &config,
        &mut state,
        FidelityDeltaMm::new(9.0, -3.0),
        ts(1_000_000_000 + long_gap_ns + 1),
    )
    .unwrap();
    assert_eq!(outcome, FidelityOutcome::Reanchored);
    assert_eq!(state.pending_dead_zone_mm(), (0.0, 0.0));
    assert_eq!(state.filtered_velocity_mm_per_s(), 0.0);
    assert_eq!(
        state.last_sample_timestamp(),
        Some(ts(1_000_000_000 + long_gap_ns + 1))
    );
}

#[test]
fn long_gap_reanchor_is_not_an_error() {
    // Reanchored is a normal policy outcome; process returns Ok.
    let config = base_config();
    let mut state = FidelityState::fresh();
    let long_gap_ns = config.long_gap().as_nanos() as u64;
    let _ = process(
        &config,
        &mut state,
        FidelityDeltaMm::new(0.0, 0.0),
        ts(1_000_000_000),
    )
    .unwrap();
    let result = process(
        &config,
        &mut state,
        FidelityDeltaMm::new(0.0, 0.0),
        ts(1_000_000_000 + long_gap_ns),
    );
    assert_eq!(result, Ok(FidelityOutcome::Reanchored));
}

// ---------------------------------------------------------------------------
// Signed radial dead zone: cancellation, slow release, reversals, diagonals
// ---------------------------------------------------------------------------

#[test]
fn signed_oscillation_cancels_algebraically() {
    let config = base_config();
    let mut state = FidelityState::fresh();
    // Back-and-forth jitter around the anchor at realistic 8 ms spacing:
    // +0.05, -0.05, +0.05, -0.05 mm. Every frame is below the 0.09 mm radius
    // and the signed displacement cancels algebraically in P, so nothing is
    // ever emitted.
    let frame_ns = 8_000_000u64;
    for (i, delta) in [(0.05, 0.0), (-0.05, 0.0), (0.05, 0.0), (-0.05, 0.0)]
        .into_iter()
        .enumerate()
    {
        let outcome = process(
            &config,
            &mut state,
            FidelityDeltaMm::new(delta.0, delta.1),
            ts(1_000_000_000 + i as u64 * frame_ns),
        )
        .unwrap();
        assert_eq!(outcome, FidelityOutcome::Hold, "frame {i}");
    }
    // The oscillation cancelled to (approximately) zero; the dead zone held.
    let (px, py) = state.pending_dead_zone_mm();
    assert!(px.abs() < 1e-12 && py.abs() < 1e-12, "P = ({px}, {py})");
    // The velocity estimator may carry the small jitter speed, but P — the
    // dead-zone accumulator — must have cancelled exactly.
    assert!(state.filtered_velocity_mm_per_s().is_finite());
}

#[test]
fn slow_consistent_motion_waits_for_the_radius_then_releases_all() {
    let config = base_config();
    let mut state = FidelityState::fresh();
    // 0.03 mm per 10 ms frame: below the 0.09 mm radius each frame.
    // P accumulates 0.03, 0.06, 0.09 — the third frame crosses the radius
    // and must release the whole accumulated signed displacement (0.09 mm),
    // not merely the latest frame's delta.
    let mut held = 0;
    for i in 0..4 {
        let outcome = process(
            &config,
            &mut state,
            FidelityDeltaMm::new(0.03, 0.0),
            ts(1_000_000_000 + i * 10_000_000),
        )
        .unwrap();
        match outcome {
            FidelityOutcome::Hold => held += 1,
            FidelityOutcome::EmitScaledPixels { x, .. } => {
                // All 0.09 mm of accumulated signed displacement released.
                assert!(
                    (x - 0.09 * scalar(&config, state.filtered_velocity_mm_per_s())).abs() < 1e-9
                );
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    // Frame 0 anchors and holds (P = 0.03); frame 1 holds (P = 0.06);
    // frame 2 releases all of P (0.09 >= radius); frame 3 holds again
    // (P = 0.03). So exactly three frames hold.
    assert_eq!(held, 3, "frames 0,1,3 hold; frame 2 releases all of P");
    assert_eq!(state.pending_dead_zone_mm(), (0.03, 0.0));
}

#[test]
fn diagonal_motion_releases_both_axes_together() {
    let config = base_config();
    let mut state = FidelityState::fresh();
    // Diagonal: equal positive x and y. The radial norm crosses the radius
    // and both axes are emitted with the same isotropic scalar.
    let outcome = process(
        &config,
        &mut state,
        FidelityDeltaMm::new(0.07, 0.07),
        ts(1_000_000_000),
    )
    .unwrap();
    // norm = 0.09899 >= 0.09 -> emit both axes.
    match outcome {
        FidelityOutcome::EmitScaledPixels { x, y } => {
            let s = scalar(&config, 0.0);
            assert!((x - 0.07 * s).abs() < 1e-9);
            assert!((y - 0.07 * s).abs() < 1e-9);
        }
        other => panic!("expected diagonal emission, got {other:?}"),
    }
}

#[test]
fn reversal_after_accumulation_cancels_before_release() {
    let config = base_config();
    let mut state = FidelityState::fresh();
    // Accumulate 0.08 mm (below radius), then reverse by -0.08 mm: P returns
    // to zero, so no release ever happens.
    let _ = process(
        &config,
        &mut state,
        FidelityDeltaMm::new(0.08, 0.0),
        ts(1_000_000_000),
    )
    .unwrap();
    let second = process(
        &config,
        &mut state,
        FidelityDeltaMm::new(-0.08, 0.0),
        ts(1_010_000_000),
    )
    .unwrap();
    assert_eq!(second, FidelityOutcome::Hold);
    let (px, _) = state.pending_dead_zone_mm();
    assert!(px.abs() < 1e-12, "reversal must cancel P, got {px}");
}

#[test]
fn release_uses_the_current_filtered_velocity_not_the_first_frame() {
    // After warm-up at 100 mm/s, the gain curve is above min_gain: a release
    // after warm-up must use the *current* filtered velocity.
    let config = base_config();
    let mut state = FidelityState::fresh();
    let dt = 10_000_000u64;
    let delta = 1.0; // 100 mm/s
    let _ = process(&config, &mut state, FidelityDeltaMm::new(delta, 0.0), ts(0)).unwrap();
    for i in 1..=200 {
        let _ = process(
            &config,
            &mut state,
            FidelityDeltaMm::new(delta, 0.0),
            ts(i * dt),
        )
        .unwrap();
    }
    let v = state.filtered_velocity_mm_per_s();
    assert!((v - 100.0).abs() < 0.1);
    // Now a below-radius delta is held and then pushed over the radius: the
    // release uses the current (warm) velocity.
    let _ = process(
        &config,
        &mut state,
        FidelityDeltaMm::new(0.04, 0.0),
        ts(201 * dt),
    )
    .unwrap();
    assert_eq!(state.pending_dead_zone_mm(), (0.04, 0.0));
    let release = process(
        &config,
        &mut state,
        FidelityDeltaMm::new(0.05, 0.0),
        ts(211 * dt),
    )
    .unwrap();
    let current_v = state.filtered_velocity_mm_per_s();
    match release {
        FidelityOutcome::EmitScaledPixels { x, .. } => {
            assert!(
                (x - 0.09 * scalar(&config, current_v)).abs() < 1e-6,
                "release must use the current filtered velocity"
            );
        }
        other => panic!("expected release, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Runtime failures
// ---------------------------------------------------------------------------

#[test]
fn non_finite_delta_is_a_runtime_error() {
    let config = base_config();
    let mut state = FidelityState::fresh();
    let result = process(
        &config,
        &mut state,
        FidelityDeltaMm::new(f64::NAN, 0.0),
        ts(1_000_000_000),
    );
    assert_eq!(result, Err(FidelityError::NonFinite));
    // A NaN y likewise fails closed.
    let result2 = process(
        &config,
        &mut state,
        FidelityDeltaMm::new(0.0, f64::INFINITY),
        ts(1_000_000_000),
    );
    assert_eq!(result2, Err(FidelityError::Overflow));
}

#[test]
fn overflowing_scaled_emission_is_a_runtime_error() {
    let config = base_config();
    let mut state = FidelityState::fresh();
    // A first-call delta whose scaled emission overflows f64: 1e308 mm *
    // scalar (>= 10) is infinite. The stage must fail closed with Overflow.
    let result = process(
        &config,
        &mut state,
        FidelityDeltaMm::new(1e308, 0.0),
        ts(1_000_000_000),
    );
    assert_eq!(result, Err(FidelityError::Overflow));
}

#[test]
fn overflowing_velocity_sample_is_a_runtime_error() {
    let config = base_config();
    let mut state = FidelityState::fresh();
    // Anchor with a sub-radius delta.
    let _ = process(
        &config,
        &mut state,
        FidelityDeltaMm::new(0.05, 0.0),
        ts(1_000_000_000),
    )
    .unwrap();
    // A huge finite displacement over 1 ns makes norm(V)/t_acc overflow:
    // fail closed with Overflow (never an infinite velocity).
    let result = process(
        &config,
        &mut state,
        FidelityDeltaMm::new(1e308, 0.0),
        ts(1_000_000_001),
    );
    assert_eq!(result, Err(FidelityError::Overflow));
}

// ---------------------------------------------------------------------------
// Sample-rate independence: 60 Hz vs 120 Hz within 1%
// ---------------------------------------------------------------------------

/// Feeds the same constant physical motion at `hz` for `warmup_seconds` and
/// returns the resulting fidelity state.
fn warm_up(
    config: &FidelityConfig,
    hz: u64,
    warmup_seconds: f64,
    speed_mm_per_s: f64,
) -> FidelityState {
    let mut state = FidelityState::fresh();
    let dt_nanos = (1_000_000_000.0 / hz as f64) as u64;
    let dt = Duration::from_nanos(dt_nanos);
    let delta = speed_mm_per_s * dt.as_secs_f64();
    let frames = (warmup_seconds / dt.as_secs_f64()) as u64;
    for i in 0..=frames {
        let outcome = process(
            config,
            &mut state,
            FidelityDeltaMm::new(delta, 0.0),
            ts(i * dt_nanos),
        )
        .unwrap();
        let _ = outcome;
    }
    state
}

#[test]
fn constant_motion_agrees_within_one_percent_at_60_and_120_hz() {
    let config = base_config();
    let state_60 = warm_up(&config, 60, 1.0, 80.0);
    let state_120 = warm_up(&config, 120, 1.0, 80.0);

    let v60 = state_60.filtered_velocity_mm_per_s();
    let v120 = state_120.filtered_velocity_mm_per_s();
    assert!(v60.is_finite() && v120.is_finite());
    let v_diff = (v60 - v120).abs() / v60.abs().max(v120.abs());
    assert!(
        v_diff <= 0.01,
        "filtered velocity relative difference {v_diff} must be <= 1% (v60={v60}, v120={v120})"
    );

    let g60 = gain(&config, v60);
    let g120 = gain(&config, v120);
    let g_diff = (g60 - g120).abs() / g60.abs().max(g120.abs());
    assert!(
        g_diff <= 0.01,
        "gain relative difference {g_diff} must be <= 1% (g60={g60}, g120={g120})"
    );

    let s60 = scalar(&config, v60);
    let s120 = scalar(&config, v120);
    let s_diff = (s60 - s120).abs() / s60.abs().max(s120.abs());
    assert!(
        s_diff <= 0.01,
        "scalar relative difference {s_diff} must be <= 1% (s60={s60}, s120={s120})"
    );

    // The absolute values must also be physically sensible (converged to the
    // injected 80 mm/s within a tight tolerance after 1 s of warm-up).
    assert!((v60 - 80.0).abs() < 1.0, "v60 = {v60}");
    assert!((v120 - 80.0).abs() < 1.0, "v120 = {v120}");
}

// ---------------------------------------------------------------------------
// M11Profile: exact constants, M10 inheritance, config equality
// ---------------------------------------------------------------------------

#[test]
fn m11_profile_exposes_the_exact_documented_constants() {
    let profile = M11Profile::new().expect("documented M11 constants must validate");
    assert_eq!(M11Profile::NAME, "m11-fidelity-v1");
    let fidelity = profile.fidelity_config();
    assert_eq!(fidelity.dead_zone_radius_mm(), 0.09);
    assert_eq!(fidelity.velocity_tau(), Duration::from_millis(20));
    assert_eq!(fidelity.long_gap(), Duration::from_millis(150));
    assert_eq!(fidelity.gain_x0_mm_per_s(), 50.0);
    assert_eq!(fidelity.gain_x1_mm_per_s(), 600.0);
    assert_eq!(fidelity.min_gain(), 1.0);
    assert_eq!(fidelity.max_gain(), 2.0);
    assert_eq!(
        fidelity.base_px_per_mm(),
        LogicalPixelsPerMm::try_new(10.0).unwrap()
    );
    assert_eq!(fidelity.tracking_speed(), 1.0);
}

#[test]
fn m11_profile_inherits_every_exposed_m10_value() {
    let m10 = M10Profile::new().unwrap();
    let m11 = M11Profile::new().unwrap();
    assert_eq!(m11.motion_threshold_mm(), m10.motion_threshold_mm());
    assert_eq!(m11.logical_pixels_per_mm(), m10.logical_pixels_per_mm());
    assert_eq!(m11.tap(), m10.tap());
    assert_eq!(m11.two_finger(), m10.two_finger());
    assert_eq!(m11.m10_profile(), &m10);
}

#[test]
fn m11_arbiter_config_is_exactly_m10_plus_fidelity() {
    let m10 = M10Profile::new().unwrap();
    let m11 = M11Profile::new().unwrap();

    // The M10 config carries no fidelity; the M11 config is exactly the M10
    // config with the M11 fidelity config attached — every M7-M9 value
    // identical, nothing copied.
    let m10_cfg = m10.arbiter_config();
    let m11_cfg = m11.arbiter_config();
    assert!(!m10_cfg.is_fidelity_enabled());
    assert!(m11_cfg.is_fidelity_enabled());
    assert_eq!(
        m11_cfg,
        m10_cfg.clone().with_fidelity(m11.fidelity_config().clone())
    );
    // The attached fidelity config is the exact profile config.
    assert_eq!(m11_cfg.fidelity_config(), Some(m11.fidelity_config()));
    // The exposed M7-M9 values are inherited verbatim.
    assert_eq!(m11_cfg.motion_threshold_mm(), m10_cfg.motion_threshold_mm());
    assert_eq!(
        m11_cfg.logical_pixels_per_mm(),
        m10_cfg.logical_pixels_per_mm()
    );
    assert_eq!(m11_cfg.tap_config(), m10_cfg.tap_config());
    assert_eq!(m11_cfg.two_finger_config(), m10_cfg.two_finger_config());
}

// ---------------------------------------------------------------------------
// Arbiter end-to-end integration (M11_TASK.md §5/§6/§9)
// ---------------------------------------------------------------------------

fn mm(x: f32) -> Millimeters {
    Millimeters::try_new(x).unwrap()
}

fn px(x: f32) -> LogicalPixels {
    LogicalPixels::try_new(x).unwrap()
}

fn contact(tracking_id: i32, slot: u32, state: ContactState, x: f32, y: f32) -> Contact {
    let mut c = Contact::new(tracking_id, slot, state);
    c.x_mm = Some(mm(x));
    c.y_mm = Some(mm(y));
    c
}

fn frame(
    sequence: u64,
    ts: u64,
    contacts: Vec<Contact>,
    left: bool,
    discontinuity: bool,
) -> ContactFrame {
    ContactFrame {
        monotonic_timestamp: Monotonic::from_nanos(ts),
        sequence,
        discontinuity,
        contacts,
        physical_buttons: PhysicalButtons::new(left, false, false),
        diagnostics: vec![],
    }
}

fn move_event(dx: f32, dy: f32) -> OutputEvent {
    OutputEvent::PointerMove {
        dx: px(dx),
        dy: px(dy),
    }
}

fn down() -> OutputEvent {
    OutputEvent::ButtonDown(MouseButton::Left)
}

fn up() -> OutputEvent {
    OutputEvent::ButtonUp(MouseButton::Left)
}

/// The validated `m11-fidelity-v1` arbiter configuration (M10 profile plus
/// the fidelity stage).
fn m11_cfg() -> ArbiterConfig {
    M11Profile::new().unwrap().arbiter_config()
}

/// The validated `m10-linear-v1` arbiter configuration (fidelity disabled).
fn m10_cfg() -> ArbiterConfig {
    M10Profile::new().unwrap().arbiter_config()
}

/// An arbiter configuration with the M11 fidelity stage attached but an
/// absurdly large (yet finite and positive) tracking speed, used to drive a
/// fidelity/quantization failure through the public frame API. The scale is
/// large enough that a 3e38 mm commit overflows the f32 pixel range
/// (fail-closed [`ArbiterError::NonFinite`]), yet small enough that a normal
/// 2.0 mm commit still emits a finite f32 pixel value.
fn overflow_cfg() -> ArbiterConfig {
    let fidelity = FidelityConfig::new(
        0.09,
        Duration::from_millis(20),
        Duration::from_millis(150),
        50.0,
        600.0,
        1.0,
        2.0,
        LogicalPixelsPerMm::try_new(10.0).unwrap(),
        1e37,
    )
    .unwrap();
    ArbiterConfig::new(mm(1.0), LogicalPixelsPerMm::try_new(10.0).unwrap())
        .unwrap()
        .with_fidelity(fidelity)
}

#[test]
fn fidelity_disabled_arbiter_keeps_the_exact_m10_decision_stream() {
    // The M10 profile (fidelity disabled) must produce the exact pre-M11
    // output for a representative interaction: candidate silence, exactly-one
    // accumulated commit at the linear scale, incremental continuation, final
    // clean movement before release, and the M8 tap/button behavior.
    let mut arbiter = Arbiter::new(m10_cfg());

    // Candidate period: no output.
    let d0 = arbiter
        .frame(&frame(
            1,
            1_000,
            vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert!(d0.events.is_empty());
    assert_eq!(d0.lifecycle_after, Lifecycle::Candidate);

    // Commit: 2.0 mm * 10 px/mm = 20 px, exactly once.
    let d1 = arbiter
        .frame(&frame(
            2,
            1_016,
            vec![contact(1, 0, ContactState::Active, 2.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(d1.events, vec![move_event(20.0, 0.0)]);
    assert_eq!(d1.lifecycle_after, Lifecycle::Committed);

    // Continuation: 0.5 mm * 10 px/mm = 5 px.
    let d2 = arbiter
        .frame(&frame(
            3,
            1_032,
            vec![contact(1, 0, ContactState::Active, 2.5, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(d2.events, vec![move_event(5.0, 0.0)]);

    // Final clean movement (0.125 mm -> 1.25 px, truncating to 1 px)
    // precedes the Finish transition. 2.625 is exactly representable in f32
    // (21/8), so the widened f64 delta is exactly 0.125 mm.
    let d3 = arbiter
        .frame(&frame(
            4,
            1_048,
            vec![contact(1, 0, ContactState::Ended, 2.625, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(d3.events, vec![move_event(1.0, 0.0)]);
    assert_eq!(
        d3.transitions,
        vec![LifecycleTransition::Finish { tracking_id: 1 }]
    );
    assert_eq!(arbiter.remainder_px(), (0.0, 0.0));
}

#[test]
fn candidate_ownership_decisions_are_identical_with_fidelity_enabled() {
    // Ownership/candidate decisions remain based on raw normalized
    // millimeters before the dead zone or gain: the candidate period emits
    // nothing and the commit transition happens at the same threshold with
    // both profiles (M11_TASK.md §5). The first frame is a `Began` that
    // anchors the candidate in both arbiters; the rest stay below threshold.
    let mut m10 = Arbiter::new(m10_cfg());
    let mut m11 = Arbiter::new(m11_cfg());

    let frames = [
        (1_000u64, ContactState::Began, 0.0f32),
        (1_008, ContactState::Active, 0.3),
        (1_016, ContactState::Active, 0.6),
        (1_024, ContactState::Active, 0.9),
    ];
    for (ts, state, x) in frames {
        let d10 = m10
            .frame(&frame(
                ts / 8,
                ts,
                vec![contact(1, 0, state, x, 0.0)],
                false,
                false,
            ))
            .unwrap();
        let d11 = m11
            .frame(&frame(
                ts / 8,
                ts,
                vec![contact(1, 0, state, x, 0.0)],
                false,
                false,
            ))
            .unwrap();
        assert_eq!(d10.events, d11.events, "ts {ts}: events");
        assert_eq!(d10.lifecycle_after, d11.lifecycle_after, "ts {ts}");
        assert_eq!(d10.transitions, d11.transitions, "ts {ts}");
        assert_eq!(d10.lifecycle_after, Lifecycle::Candidate);
    }
}

#[test]
fn m11_first_commit_continuation_and_final_clean_movement() {
    let mut arbiter = Arbiter::new(m11_cfg());

    // Frame 1: candidate begins, no output (pre-fidelity ownership).
    let d0 = arbiter
        .frame(&frame(
            1,
            1_000,
            vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert!(d0.events.is_empty());
    assert_eq!(d0.lifecycle_after, Lifecycle::Candidate);

    // Frame 2: commit. The whole 2.0 mm candidate displacement is the first
    // fidelity call: released at the initial filtered velocity 0 -> min_gain
    // (1.0), so 2.0 mm * 10 px/mm = 20 px.
    let d1 = arbiter
        .frame(&frame(
            2,
            1_016,
            vec![contact(1, 0, ContactState::Active, 2.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(d1.events, vec![move_event(20.0, 0.0)]);
    assert_eq!(d1.lifecycle_after, Lifecycle::Committed);

    // Frame 3: continuation. 0.5 mm at ~31.25 mm/s (0.5 mm / 16 ms) is below
    // gain_x0, so the gain is still min_gain: 0.5 mm * 10 px/mm = 5 px.
    let d2 = arbiter
        .frame(&frame(
            3,
            1_032,
            vec![contact(1, 0, ContactState::Active, 2.5, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(d2.events, vec![move_event(5.0, 0.0)]);

    // Frame 4: final clean movement (0.125 mm -> 1.25 px, truncating to
    // 1 px) then Finish; the interaction's fidelity and remainder reset after
    // the final motion. 2.625 is exactly representable in f32 (21/8), so the
    // delta from 2.5 is exactly 0.125 mm.
    let d3 = arbiter
        .frame(&frame(
            4,
            1_048,
            vec![contact(1, 0, ContactState::Ended, 2.625, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(d3.events, vec![move_event(1.0, 0.0)]);
    assert_eq!(
        d3.transitions,
        vec![LifecycleTransition::Finish { tracking_id: 1 }]
    );
    assert_eq!(arbiter.remainder_px(), (0.0, 0.0));
    assert_eq!(arbiter.lifecycle(), Lifecycle::Finished);
}

#[test]
fn m11_remainder_invariant_has_no_epsilon_drain_and_is_exposed() {
    // Fractional scaled emissions exercise the exact per-axis remainder
    // invariant: total = prior_remainder + scaled; emitted = trunc(total);
    // new_remainder = total - emitted; the remainder stays in (-1, 1) and
    // no epsilon drains. All millimeter values are exactly representable in
    // f32 (dyadic fractions), so the f64-widened arithmetic is exact.
    let mut arbiter = Arbiter::new(m11_cfg());
    let _ = arbiter
        .frame(&frame(
            1,
            1_000,
            vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    // Commit 2.25 mm at min gain: scaled 22.5 px -> emit 22, remainder 0.5.
    let d1 = arbiter
        .frame(&frame(
            2,
            1_016,
            vec![contact(1, 0, ContactState::Active, 2.25, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(d1.events, vec![move_event(22.0, 0.0)]);
    assert_eq!(arbiter.remainder_px(), (0.5, 0.0));

    // Continuation 0.25 mm at min gain: scaled 2.5 px; total = 2.5 + 0.5 =
    // 3.0 -> emit 3, remainder 0. The aggregate equals the physical motion.
    let d2 = arbiter
        .frame(&frame(
            3,
            1_032,
            vec![contact(1, 0, ContactState::Active, 2.5, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(d2.events, vec![move_event(3.0, 0.0)]);
    assert_eq!(arbiter.remainder_px(), (0.0, 0.0));

    // Continued small motion: 2.5 -> 2.625 = 0.125 mm at min gain = 1.25 px;
    // emit 1, remainder 0.25 (the fraction is retained, no epsilon drain).
    let d3 = arbiter
        .frame(&frame(
            4,
            1_048,
            vec![contact(1, 0, ContactState::Active, 2.625, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(d3.events, vec![move_event(1.0, 0.0)]);
    assert_eq!(arbiter.remainder_px(), (0.25, 0.0));

    // 2.625 -> 2.75 = 0.125 mm = 1.25 px; total = 1.25 + 0.25 = 1.5 -> emit
    // 1, remainder 0.5.
    let d4 = arbiter
        .frame(&frame(
            5,
            1_064,
            vec![contact(1, 0, ContactState::Active, 2.75, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(d4.events, vec![move_event(1.0, 0.0)]);
    assert_eq!(arbiter.remainder_px(), (0.5, 0.0));

    // 2.75 -> 2.875 = 0.125 mm = 1.25 px; total = 1.75 -> emit 1, rem 0.75.
    let d5 = arbiter
        .frame(&frame(
            6,
            1_080,
            vec![contact(1, 0, ContactState::Active, 2.875, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(d5.events, vec![move_event(1.0, 0.0)]);
    assert_eq!(arbiter.remainder_px(), (0.75, 0.0));

    // 2.875 -> 3.0 = 0.125 mm = 1.25 px; total = 2.0 -> emit 2, remainder 0:
    // every subpixel fraction is eventually delivered exactly.
    let d6 = arbiter
        .frame(&frame(
            7,
            1_096,
            vec![contact(1, 0, ContactState::Active, 3.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(d6.events, vec![move_event(2.0, 0.0)]);
    assert_eq!(arbiter.remainder_px(), (0.0, 0.0));

    // Aggregate invariant over the whole interaction: emitted px sum (22+3+1
    // +1+1+2 = 30) equals the total physical motion (3.0 mm * 10 px/mm),
    // with zero final remainder.
    let total_emitted: f32 = [
        d1.events.clone(),
        d2.events.clone(),
        d3.events.clone(),
        d4.events.clone(),
        d5.events.clone(),
        d6.events.clone(),
    ]
    .into_iter()
    .flatten()
    .filter_map(|e| match e {
        OutputEvent::PointerMove { dx, .. } => Some(dx.as_px()),
        _ => None,
    })
    .sum();
    assert_eq!(total_emitted, 30.0);
}

#[test]
fn m11_hold_leaves_remainder_unchanged() {
    let mut arbiter = Arbiter::new(m11_cfg());
    let _ = arbiter
        .frame(&frame(
            1,
            1_000,
            vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    let _ = arbiter
        .frame(&frame(
            2,
            1_016,
            vec![contact(1, 0, ContactState::Active, 2.25, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(arbiter.remainder_px(), (0.5, 0.0));

    // Sub-radius deltas are held: no pointer event, remainder untouched.
    // 2.3125 = 37/16 is f32-exact; the 0.0625 mm delta stays below the
    // 0.09 mm dead-zone radius.
    let d = arbiter
        .frame(&frame(
            3,
            1_032,
            vec![contact(1, 0, ContactState::Active, 2.3125, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert!(d.events.is_empty(), "{:?}", d.events);
    assert_eq!(arbiter.remainder_px(), (0.5, 0.0));

    // The next frame pushes P (0.125 mm) over the radius; the release
    // carries the accumulated P through the existing quantization: scaled
    // 1.25 px + 0.5 remainder = 1.75 -> emit 1, remainder 0.75.
    let d = arbiter
        .frame(&frame(
            4,
            1_048,
            vec![contact(1, 0, ContactState::Active, 2.375, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(d.events, vec![move_event(1.0, 0.0)]);
    assert_eq!(arbiter.remainder_px(), (0.75, 0.0));
}

#[test]
fn m11_reanchored_preserves_the_pixel_remainder() {
    let mut arbiter = Arbiter::new(m11_cfg());
    let _ = arbiter
        .frame(&frame(
            1,
            1_000,
            vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    let _ = arbiter
        .frame(&frame(
            2,
            1_016,
            vec![contact(1, 0, ContactState::Active, 2.25, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(arbiter.remainder_px(), (0.5, 0.0));

    // A long gap (>= 150 ms) re-anchors the fidelity stage and emits zero,
    // but is NOT a lifecycle reset: the same interaction continues and the
    // existing pixel remainder survives (M11_TASK.md §7.4/§9). 2.625 is
    // f32-exact (21/8); the gap-crossing displacement is discarded.
    let long_gap_ns = m11_cfg().fidelity_config().unwrap().long_gap().as_nanos() as u64;
    let d = arbiter
        .frame(&frame(
            3,
            1_016 + long_gap_ns,
            vec![contact(1, 0, ContactState::Active, 2.625, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert!(d.events.is_empty(), "{:?}", d.events);
    assert_eq!(arbiter.lifecycle(), Lifecycle::Committed);
    assert_eq!(
        arbiter.remainder_px(),
        (0.5, 0.0),
        "remainder survives re-anchor"
    );

    // The next positive frame folds fresh and quantizes through the
    // preserved remainder: 2.625 -> 2.875 = 0.25 mm at min gain = 2.5 px +
    // 0.5 remainder = 3.0 -> emit 3, remainder 0.
    let d = arbiter
        .frame(&frame(
            4,
            1_016 + long_gap_ns + 16_000_000,
            vec![contact(1, 0, ContactState::Active, 2.875, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(d.events, vec![move_event(3.0, 0.0)]);
    assert_eq!(arbiter.remainder_px(), (0.0, 0.0));
}

#[test]
fn rejected_runtime_error_frame_rolls_back_remainder_and_fidelity_state() {
    let mut arbiter = Arbiter::new(overflow_cfg());
    let _ = arbiter
        .frame(&frame(
            1,
            1_000,
            vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(arbiter.lifecycle(), Lifecycle::Candidate);

    // The commit delta (3e38 mm) times the absurd tracking multiplier
    // overflows the f32 pixel range in the pointer path: the frame must fail
    // closed with the existing ArbiterError::NonFinite, and NO partial state
    // may commit — no events, no lifecycle change, no remainder change.
    let huge = 3e38_f32;
    let rejected = arbiter.frame(&frame(
        2,
        1_016,
        vec![contact(1, 0, ContactState::Active, huge, 0.0)],
        false,
        false,
    ));
    assert!(matches!(
        rejected,
        Err(ArbiterError::NonFinite { sequence: 2 })
    ));
    assert_eq!(arbiter.lifecycle(), Lifecycle::Candidate);
    assert_eq!(arbiter.remainder_px(), (0.0, 0.0));

    // The arbiter is unharmed: a subsequent valid frame commits normally —
    // accepted (not rejected) at the absurd tracking scale, so the emitted
    // px is huge but finite — proving the rejected frame applied nothing.
    let d = arbiter
        .frame(&frame(
            3,
            1_032,
            vec![contact(1, 0, ContactState::Active, 2.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(d.lifecycle_after, Lifecycle::Committed);
    assert!(
        matches!(d.events.as_slice(), [OutputEvent::PointerMove { dx, dy }] if dy.as_px() == 0.0 && dx.as_px() > 0.0),
        "{:?}",
        d.events
    );
}

#[test]
fn timestamp_regression_occurs_before_fidelity_and_applies_no_partial_draft() {
    let mut arbiter = Arbiter::new(m11_cfg());
    let _ = arbiter
        .frame(&frame(
            1,
            1_000,
            vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    let _ = arbiter
        .frame(&frame(
            2,
            1_016,
            vec![contact(1, 0, ContactState::Active, 2.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(arbiter.lifecycle(), Lifecycle::Committed);

    // The regressed frame is rejected by the existing timestamp regression
    // check, BEFORE any fidelity code runs: the arbiter cancels the active
    // interaction fail-closed and emits no pointer event from this frame.
    let rejected = arbiter.frame(&frame(
        3,
        1_000, // regression: earlier than the last accepted timestamp
        vec![contact(1, 0, ContactState::Active, 5.0, 0.0)],
        false,
        false,
    ));
    assert!(matches!(
        rejected,
        Err(ArbiterError::TimestampRegression { .. })
    ));
    assert_eq!(arbiter.lifecycle(), Lifecycle::Cancelled);
    assert_eq!(arbiter.remainder_px(), (0.0, 0.0));

    // A genuinely new interaction starts with completely fresh fidelity: its
    // first commit is at min gain (no leftover velocity/accumulators).
    let _ = arbiter
        .frame(&frame(
            4,
            2_000,
            vec![contact(2, 1, ContactState::Began, 0.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    let d = arbiter
        .frame(&frame(
            5,
            2_016,
            vec![contact(2, 1, ContactState::Active, 2.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(d.events, vec![move_event(20.0, 0.0)]);
}

#[test]
fn sequence_regression_occurs_before_fidelity() {
    let mut arbiter = Arbiter::new(m11_cfg());
    let _ = arbiter
        .frame(&frame(
            1,
            1_000,
            vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    let _ = arbiter
        .frame(&frame(
            2,
            1_016,
            vec![contact(1, 0, ContactState::Active, 2.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    let rejected = arbiter.frame(&frame(
        2, // sequence regression: not strictly greater
        1_032,
        vec![contact(1, 0, ContactState::Active, 3.0, 0.0)],
        false,
        false,
    ));
    assert!(matches!(
        rejected,
        Err(ArbiterError::SequenceRegression { .. })
    ));
    assert_eq!(arbiter.lifecycle(), Lifecycle::Cancelled);
    assert_eq!(arbiter.remainder_px(), (0.0, 0.0));
}

#[test]
fn clean_end_resets_fidelity_and_remainder_after_final_motion() {
    let mut arbiter = Arbiter::new(m11_cfg());
    let _ = arbiter
        .frame(&frame(
            1,
            1_000,
            vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    // Commit with a fractional remainder so reset observability is real.
    let _ = arbiter
        .frame(&frame(
            2,
            1_016,
            vec![contact(1, 0, ContactState::Active, 2.25, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(arbiter.remainder_px(), (0.5, 0.0));
    // A held sub-radius delta (0.0625 mm, f32-exact 37/16) accumulates
    // pending fidelity state.
    let _ = arbiter
        .frame(&frame(
            3,
            1_032,
            vec![contact(1, 0, ContactState::Active, 2.3125, 0.0)],
            false,
            false,
        ))
        .unwrap();

    // Clean end: the final committed movement is processed first, then the
    // interaction finishes and fidelity + pixel remainder reset.
    let d = arbiter
        .frame(&frame(
            4,
            1_048,
            vec![contact(1, 0, ContactState::Ended, 2.375, 0.0)],
            false,
            false,
        ))
        .unwrap();
    // 0.0625 mm folds into P (0.125 >= 0.09) -> 1.25 px + 0.5 rem = 1.75
    // -> emit 1; then the interaction finishes and the remainder resets.
    assert_eq!(d.events, vec![move_event(1.0, 0.0)]);
    assert_eq!(
        d.transitions,
        vec![LifecycleTransition::Finish { tracking_id: 1 }]
    );
    assert_eq!(arbiter.remainder_px(), (0.0, 0.0));
    assert_eq!(arbiter.lifecycle(), Lifecycle::Finished);

    // A fresh interaction starts with zeroed fidelity and remainder.
    let _ = arbiter
        .frame(&frame(
            5,
            2_000,
            vec![contact(2, 1, ContactState::Began, 0.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    let d = arbiter
        .frame(&frame(
            6,
            2_016,
            vec![contact(2, 1, ContactState::Active, 2.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(d.events, vec![move_event(20.0, 0.0)]);
}

#[test]
fn tracking_id_replacement_processes_old_final_motion_then_starts_fresh() {
    let mut arbiter = Arbiter::new(m11_cfg());
    let _ = arbiter
        .frame(&frame(
            1,
            1_000,
            vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    let _ = arbiter
        .frame(&frame(
            2,
            1_016,
            vec![contact(1, 0, ContactState::Active, 2.25, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(arbiter.remainder_px(), (0.5, 0.0));

    // Replacement frame: the old contact ends with final committed motion and
    // a new contact begins in the same frame. The old final motion is
    // processed through fidelity first (2.25 -> 2.3125 = 0.0625 mm, held
    // below the dead-zone radius), then the old interaction resets (fidelity
    // + remainder), then the new contact begins with fresh state.
    let d = arbiter
        .frame(&frame(
            3,
            1_032,
            vec![
                contact(1, 0, ContactState::Ended, 2.3125, 0.0),
                contact(2, 1, ContactState::Began, 0.0, 0.0),
            ],
            false,
            false,
        ))
        .unwrap();
    assert!(d.events.is_empty());
    assert_eq!(
        d.transitions,
        vec![
            LifecycleTransition::Finish { tracking_id: 1 },
            LifecycleTransition::Begin { tracking_id: 2 },
        ]
    );
    assert_eq!(arbiter.remainder_px(), (0.0, 0.0));
    assert_eq!(arbiter.lifecycle(), Lifecycle::Candidate);

    // The new contact's first commit is at min gain with fresh fidelity.
    let d = arbiter
        .frame(&frame(
            4,
            1_048,
            vec![contact(2, 1, ContactState::Active, 2.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(d.events, vec![move_event(20.0, 0.0)]);
    assert_eq!(d.lifecycle_after, Lifecycle::Committed);
}

#[test]
fn discontinuity_cancels_before_contact_handling_with_no_final_motion() {
    let mut arbiter = Arbiter::new(m11_cfg());
    let _ = arbiter
        .frame(&frame(
            1,
            1_000,
            vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    let _ = arbiter
        .frame(&frame(
            2,
            1_016,
            vec![contact(1, 0, ContactState::Active, 2.25, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(arbiter.remainder_px(), (0.5, 0.0));

    // A discontinuity frame cancels the interaction BEFORE contact handling:
    // no final pointer motion from the cancelled interaction, no events,
    // fidelity and remainder reset.
    let d = arbiter
        .frame(&frame(
            3,
            1_032,
            vec![contact(1, 0, ContactState::Active, 3.0, 0.0)],
            false,
            true,
        ))
        .unwrap();
    assert!(d.events.is_empty());
    assert!(d
        .transitions
        .iter()
        .any(|t| matches!(t, LifecycleTransition::Cancel { tracking_id: 1 })));
    assert_eq!(arbiter.lifecycle(), Lifecycle::Cancelled);
    assert_eq!(arbiter.remainder_px(), (0.0, 0.0));
}

#[test]
fn cancellation_by_second_contact_discards_pending_fidelity_motion() {
    let mut arbiter = Arbiter::new(m11_cfg());
    let _ = arbiter
        .frame(&frame(
            1,
            1_000,
            vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    // A held sub-radius delta accumulates pending P (0.05 mm).
    let _ = arbiter
        .frame(&frame(
            2,
            1_016,
            vec![contact(1, 0, ContactState::Active, 2.05, 0.0)],
            false,
            false,
        ))
        .unwrap();
    // 2.05 -> 2.10 = 0.05 mm: held, no emission, P = 0.05.
    let _ = arbiter
        .frame(&frame(
            3,
            1_032,
            vec![contact(1, 0, ContactState::Active, 2.10, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert!(arbiter.remainder_px().0 > 0.0);

    // A second live contact cancels the one-finger interaction: pending
    // fidelity motion (P) is discarded and the state resets.
    let d = arbiter
        .frame(&frame(
            4,
            1_048,
            vec![
                contact(1, 0, ContactState::Active, 2.10, 0.0),
                contact(2, 1, ContactState::Began, 5.0, 5.0),
            ],
            false,
            false,
        ))
        .unwrap();
    assert!(d.events.is_empty());
    assert!(d
        .transitions
        .iter()
        .any(|t| matches!(t, LifecycleTransition::Cancel { tracking_id: 1 })));
    assert_eq!(arbiter.remainder_px(), (0.0, 0.0));
    assert_eq!(arbiter.lifecycle(), Lifecycle::Cancelled);
}

#[test]
fn release_all_resets_fidelity_remainder_and_all_interaction_state() {
    let mut arbiter = Arbiter::new(m11_cfg());
    let _ = arbiter
        .frame(&frame(
            1,
            1_000,
            vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    let _ = arbiter
        .frame(&frame(
            2,
            1_016,
            vec![contact(1, 0, ContactState::Active, 2.25, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(arbiter.remainder_px(), (0.5, 0.0));

    let events = arbiter.release_all();
    assert!(events.is_empty(), "no held buttons to release: {events:?}");
    assert_eq!(arbiter.lifecycle(), Lifecycle::Idle);
    assert_eq!(arbiter.remainder_px(), (0.0, 0.0));

    // A fresh interaction after release_all starts with zeroed fidelity.
    let _ = arbiter
        .frame(&frame(
            3,
            2_000,
            vec![contact(2, 1, ContactState::Began, 0.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    let d = arbiter
        .frame(&frame(
            4,
            2_016,
            vec![contact(2, 1, ContactState::Active, 2.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(d.events, vec![move_event(20.0, 0.0)]);
}

#[test]
fn tap_and_tap_drag_ownership_stays_pre_fidelity() {
    // M11 must not change tap/tap-drag ownership: a qualifying tap establishes
    // the deferred press at release, and a follow-up contact within the
    // window reuses that press if pointer motion commits to tap-and-drag.
    let mut arbiter = Arbiter::new(m11_cfg());
    // Quick tap: begin, small movement below the pointer threshold, end.
    let _ = arbiter
        .frame(&frame(
            1,
            1_000,
            vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    let _ = arbiter
        .frame(&frame(
            2,
            1_100,
            vec![contact(1, 0, ContactState::Active, 0.4, 0.2)],
            false,
            false,
        ))
        .unwrap();
    let d = arbiter
        .frame(&frame(
            3,
            1_150,
            vec![contact(1, 0, ContactState::Ended, 0.4, 0.2)],
            false,
            false,
        ))
        .unwrap();
    // The sub-threshold motion never reached the fidelity stage; only the
    // deferred press is exposed until the follow-up window resolves.
    assert_eq!(d.events, vec![down()]);
    assert_eq!(d.lifecycle_after, Lifecycle::Finished);
}

#[test]
fn two_finger_scroll_ownership_stays_pre_fidelity() {
    // Two-finger scroll competes on raw normalized millimeters: the second
    // finger cancels the one-finger interaction, and the scroll lifecycle
    // emits ScrollBegin/Delta/End unchanged (M11 never touches it).
    let mut arbiter = Arbiter::new(m11_cfg());
    let _ = arbiter
        .frame(&frame(
            1,
            1_000,
            vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    // Second finger appears: one-finger interaction cancelled; the two-finger
    // candidate anchors.
    let _ = arbiter
        .frame(&frame(
            2,
            1_100,
            vec![
                contact(1, 0, ContactState::Active, 0.0, 0.0),
                contact(2, 1, ContactState::Began, 5.0, 5.0),
            ],
            false,
            false,
        ))
        .unwrap();
    // Two-finger motion commits the scroll: the centroid moved 1.0 mm on
    // each axis (>= the M10 profile's 1.0 mm scroll commit threshold) ->
    // 10 px per axis at the 10 px/mm scroll scale.
    let d = arbiter
        .frame(&frame(
            3,
            1_200,
            vec![
                contact(1, 0, ContactState::Active, 1.0, 1.0),
                contact(2, 1, ContactState::Active, 6.0, 6.0),
            ],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(
        d.events,
        vec![OutputEvent::ScrollBegin, scroll_delta(10.0, 10.0)]
    );
}

fn scroll_delta(dx: f32, dy: f32) -> OutputEvent {
    OutputEvent::ScrollDelta {
        dx: px(dx),
        dy: px(dy),
    }
}

#[test]
fn physical_button_ownership_stays_pre_fidelity() {
    let mut arbiter = Arbiter::new(m11_cfg());
    // A physical left press while a candidate is active emits ButtonDown
    // immediately (pre-fidelity button ownership), even though the candidate
    // has not crossed the pointer threshold.
    let d = arbiter
        .frame(&frame(
            1,
            1_000,
            vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
            true,
            false,
        ))
        .unwrap();
    assert_eq!(d.events, vec![down()]);
    // The release emits exactly one ButtonUp.
    let d = arbiter
        .frame(&frame(
            2,
            1_100,
            vec![contact(1, 0, ContactState::Active, 0.3, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(d.events, vec![up()]);
}

#[test]
fn m8_follow_up_drag_committed_motion_flows_through_fidelity() {
    // M8 tap-and-drag follow-up: a new contact inside the follow-up window
    // stays pending until pointer commitment; committed pointer movement goes
    // through the same one-finger fidelity machinery (M11_TASK.md §5).
    let mut arbiter = Arbiter::new(m11_cfg());
    let _ = arbiter
        .frame(&frame(
            1,
            1_000,
            vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    let _ = arbiter
        .frame(&frame(
            2,
            1_100,
            vec![contact(1, 0, ContactState::Active, 0.3, 0.2)],
            false,
            false,
        ))
        .unwrap();
    let _ = arbiter
        .frame(&frame(
            3,
            1_150,
            vec![contact(1, 0, ContactState::Ended, 0.3, 0.2)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(arbiter.tap_drag_phase(), TapDragPhase::FollowUpWindow);

    let _ = arbiter
        .frame(&frame(
            4,
            1_300,
            vec![contact(2, 1, ContactState::Began, 0.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(arbiter.tap_drag_phase(), TapDragPhase::TapDragCandidate);
    assert!(arbiter.is_synthetic_left_held());

    let d = arbiter
        .frame(&frame(
            5,
            1_316,
            vec![contact(2, 1, ContactState::Active, 2.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(d.events, vec![move_event(20.0, 0.0)]);
    assert_eq!(arbiter.tap_drag_phase(), TapDragPhase::TapDragContact);
    assert!(arbiter.is_synthetic_left_held());
}

#[test]
fn drag_lock_ownership_stays_pre_fidelity() {
    // M8 sticky drag lock is decided pre-fidelity: a committed tap-drag that
    // lifts with drag lock enabled keeps synthetic left held and enters
    // LockedWithoutContact (M11 never touches the M8 ownership).
    let mut arbiter = Arbiter::new(m11_cfg());
    // Qualifying tap -> follow-up window.
    let _ = arbiter
        .frame(&frame(
            1,
            1_000,
            vec![contact(1, 0, ContactState::Began, 0.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    let _ = arbiter
        .frame(&frame(
            2,
            1_100,
            vec![contact(1, 0, ContactState::Active, 0.3, 0.2)],
            false,
            false,
        ))
        .unwrap();
    let _ = arbiter
        .frame(&frame(
            3,
            1_150,
            vec![contact(1, 0, ContactState::Ended, 0.3, 0.2)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(arbiter.tap_drag_phase(), TapDragPhase::FollowUpWindow);

    // Follow-up contact stays pending while reusing the deferred synthetic
    // press created by the qualifying first tap.
    let _ = arbiter
        .frame(&frame(
            4,
            1_300,
            vec![contact(2, 1, ContactState::Began, 0.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(arbiter.tap_drag_phase(), TapDragPhase::TapDragCandidate);
    assert!(arbiter.is_synthetic_left_held());

    // The follow-up contact commits a real drag (through fidelity, min gain),
    // reusing the deferred press for the first committed move.
    let d = arbiter
        .frame(&frame(
            5,
            1_316,
            vec![contact(2, 1, ContactState::Active, 2.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(d.events, vec![move_event(20.0, 0.0)]);
    assert_eq!(arbiter.tap_drag_phase(), TapDragPhase::TapDragContact);
    assert!(arbiter.is_synthetic_left_held());

    // The contact lifts: a real drag with drag lock enabled enters
    // LockedWithoutContact and the synthetic left stays held — no up, no
    // additional output (the final zero delta emits nothing).
    let d = arbiter
        .frame(&frame(
            6,
            1_332,
            vec![contact(2, 1, ContactState::Ended, 2.0, 0.0)],
            false,
            false,
        ))
        .unwrap();
    assert!(d.events.is_empty(), "{:?}", d.events);
    assert_eq!(arbiter.tap_drag_phase(), TapDragPhase::LockedWithoutContact);
    assert!(
        arbiter.is_synthetic_left_held(),
        "drag lock keeps left held"
    );

    // release_all is the unconditional escape path: exactly one left up.
    let events = arbiter.release_all();
    assert_eq!(events, vec![up()]);
    assert_eq!(arbiter.tap_drag_phase(), TapDragPhase::Idle);
}

#[test]
fn two_finger_secondary_tap_ownership_stays_pre_fidelity() {
    // M9 two-finger secondary tap is decided pre-fidelity: a qualifying
    // two-finger interaction ending with clean Ended records emits exactly
    // one right click pair (M11 never touches it).
    let mut arbiter = Arbiter::new(m11_cfg());
    // Both fingers begin on the same frame: the two-finger candidate anchors.
    let d = arbiter
        .frame(&frame(
            1,
            1_000,
            vec![
                contact(1, 0, ContactState::Began, 0.0, 0.0),
                contact(2, 1, ContactState::Began, 5.0, 5.0),
            ],
            false,
            false,
        ))
        .unwrap();
    assert!(d.events.is_empty(), "{:?}", d.events);

    // Quick release with clean Ended records from both anchored members: a
    // qualifying secondary tap fires exactly ButtonDown(Right), ButtonUp(Right).
    let d = arbiter
        .frame(&frame(
            2,
            1_050,
            vec![
                contact(1, 0, ContactState::Ended, 0.1, 0.1),
                contact(2, 1, ContactState::Ended, 5.1, 5.1),
            ],
            false,
            false,
        ))
        .unwrap();
    assert_eq!(
        d.events,
        vec![
            OutputEvent::ButtonDown(MouseButton::Right),
            OutputEvent::ButtonUp(MouseButton::Right),
        ]
    );
    assert_eq!(arbiter.two_finger_phase(), TwoFingerPhase::Finished);
}
