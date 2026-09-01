//! Interaction Arbiter — M7/M8/M9 offline, platform-independent interaction
//! layer.
//!
//! The arbiter is the **single policy owner** for competition between
//! interaction families (PHASE2_PLAN.md §5 M7/M8/M9). Every normalized
//! [`ContactFrame`] enters exactly one [`Arbiter`]; there are no independent
//! pointer/tap/scroll recognizers that each commit against the same frame.
//! The arbiter owns the observable lifecycle
//! ([`Lifecycle::Candidate`] / [`Lifecycle::Committed`] /
//! [`Lifecycle::Cancelled`] / [`Lifecycle::Finished`]), the **one-finger
//! linear pointer** and the **physical left-button** lifecycle (M7), the
//! configurable **tap-to-click, tap-and-drag, and sticky drag lock** policy
//! (M8), and the configurable **two-finger two-dimensional pixel scroll,
//! secondary tap, and buttonpad two-finger physical secondary click** policy
//! (M9).
//!
//! # Scope and platform independence
//!
//! This module lives in `touchpad-core` and depends only on the core types
//! (`contact`, `output`, `units`, `time`, `diagnostic`). It must never
//! depend on Linux, evdev, KDE, Wayland, portal, libei, or desktop crates,
//! and it never instantiates any real `OutputSink` — production code drives
//! it with synthetic or trace-derived frames and reads the resulting
//! [`FrameDecision`]; tests may feed decisions to a
//! [`RecordingSink`](crate::output::RecordingSink) or a fault-injecting fake
//! through [`ArbiterSink`]. No `unsafe` is used anywhere in this module.
//!
//! # Lifecycle
//!
//! ```text
//! Idle ──begin──▶ Candidate ──commit──▶ Committed
//!                  │  │                   │
//!              finish  cancel          finish  cancel
//!                  ▼  ▼                   ▼  ▼
//!               Finished               Cancelled
//!                  │  ▲                   │  ▲
//!                  └──┴──begin────────────┘  │
//!                          (next interaction)
//! ```
//!
//! * `Idle` — no interaction. (The arbiter's resting state; not one of the
//!   four named interaction states, but required as the machine's origin.)
//! * `Candidate` — a one-finger pointer/tap-family candidate. **No output is
//!   produced while a candidate is below its motion threshold**: no
//!   `PointerMove` and no synthetic button event leak out of the candidate
//!   period.
//! * `Committed` — the candidate crossed the configured motion threshold;
//!   relative `PointerMove`s are emitted from now on.
//! * `Cancelled` — the interaction was cancelled (second live contact,
//!   discontinuity, missing required coordinates, timestamp/sequence
//!   regression); no further pointer movement is emitted from it.
//! * `Finished` — the interaction ended cleanly because its contact ended.
//!
//! Transitions are validated by [`Arbiter::validate_transition`], which
//! returns a structured [`TransitionError`] for illegal pairs (e.g.
//! `Idle -> Committed`, `Cancelled -> Finished`) instead of panicking. The
//! arbiter's own frame processing only performs legal transitions; the pure
//! validator is the observable contract and is exhaustively tested.
//!
//! # One-finger linear pointer
//!
//! M7 requires an explicit, validated configuration: a motion threshold
//! ([`ArbiterConfig::new`] rejects non-positive thresholds) and a linear
//! [`LogicalPixelsPerMm`] scale (rejects non-finite and non-positive values
//! at construction). No system/KDE settings are read and no macOS-style
//! acceleration curve is claimed — M11 owns acceleration and jitter.
//!
//! Positions and deltas remain `Millimeters` on input; semantic output is
//! `LogicalPixels`; raw counts never enter this layer. All arithmetic is
//! checked: overflow or non-finite results fail closed with a structured
//! [`ArbiterError`] and **no partial batch** (the whole frame decision is
//! computed against a draft state and committed atomically, see
//! [`Arbiter::frame`]).
//!
//! ## Sub-pixel remainder invariant
//!
//! Each axis carries a fractional remainder in *pixel* space. For a per-axis
//! displacement `d_mm` and scale `s` (px/mm), the conversion is:
//!
//! ```text
//! total        = remainder + d_mm * s     (exact f64 arithmetic)
//! emitted      = trunc(total)             (whole pixels, toward zero)
//! remainder'   = total - emitted          (always in (-1, 1))
//! ```
//!
//! **Invariant:** at every step, `Σ emitted + remainder == Σ (d_mm * s)`
//! exactly in f64 arithmetic (modulo the last ULP of floating-point
//! rounding), so the total emitted over a sequence of many small deltas
//! equals the total emitted for one equivalent aggregate delta whenever the
//! scaled totals agree. The remainder is **reset** when an interaction is
//! cancelled, finishes, or is released (`release_all`), and a new interaction
//! always begins with a zeroed remainder — residue from one contact can never
//! leak into another (tracking-id replacement and slot reuse are covered by
//! tests). The first committed movement accounts exactly once for the
//! displacement accumulated since the candidate anchor: the candidate phase
//! accumulates millimetres from the anchor and the commit quantizes that
//! accumulated displacement once (neither lost nor emitted twice).
//!
//! # Physical left-button lifecycle
//!
//! [`ContactFrame::physical_buttons`] edges are consumed atomically with the
//! frame: exactly one `ButtonDown(Left)` on `false -> true` and exactly one
//! `ButtonUp(Left)` on `true -> false`; stable state emits nothing. Repeated
//! down/up pairs pass through in frame order without artificial delay or
//! invented desktop events (two pairs are the physical double-click
//! representation). While left is held and one-finger motion is committed,
//! emitted movement represents a physical drag. Same-frame ordering is
//! deterministic: **press precedes movement, final movement precedes
//! release**.
//!
//! Button release is **never** suppressed by contact cancellation, added
//! fingers, missing touch coordinates, or discontinuity — button edges are
//! processed even on frames that cancel the pointer interaction. The
//! idempotent [`Arbiter::release_all`] is the M10 shutdown path: it releases
//! a logically held left button (matching `ButtonUp(Left)` exactly once) and
//! clears the candidate, residue, and regression baseline, even after prior
//! errors.
//!
//! # Physical/synthetic left-button arbitration (M8)
//!
//! M8 refactors the single `held_left` assumption into source-aware policy:
//! physical-left state (driven by `ContactFrame.physical_buttons`) and
//! synthetic-left state (driven by the tap/tap-drag/drag-lock policy) are
//! tracked separately and expose only their **logical OR** to the output
//! sink. `ButtonDown(Left)` is emitted only on an aggregate `false -> true`
//! transition and `ButtonUp(Left)` only on `true -> false`, so a physical
//! press during a synthetic drag/lock never produces a duplicate down, and
//! ending the synthetic source never emits an up while physical left is
//! still held. The same-frame synthetic tap pulse (down then up) still
//! produces both events even though the aggregate begins and ends false.
//! Physical release is never suppressed by tap cancellation, extra contacts,
//! missing coordinates, or discontinuity — it stays observable through the
//! aggregate once the synthetic source no longer holds it.
//!
//! M8's tap/tap-drag/drag-lock policy is documented in the
//! [`TapConfig`] and [`TapDragPhase`] items; boundary policy is: equality at
//! the configured duration/distance/gap is accepted, strictly greater
//! expires/cancels, and timeouts are evaluated only at incoming frame
//! boundaries using [`ContactFrame::monotonic_timestamp`] and checked
//! `Duration` arithmetic (never a wall clock or process-local clock).
//! Sticky drag lock has no autonomous timeout in M8;
//! [`Arbiter::release_all`] is the unconditional escape path.
//!
//! # Two-finger scroll, secondary tap, and buttonpad physical secondary
//! click (M9)
//!
//! M9's policy is documented in the [`TwoFingerConfig`] and
//! [`TwoFingerPhase`] items. Exactly two **complete** live contacts form a
//! two-finger candidate; the frame where the second valid contact appears
//! anchors the interaction, and **no pointer, button, or scroll event leaks
//! during the candidate period**. Entering the two-finger family
//! deterministically cancels/finishes any incompatible one-finger pointer
//! interaction and releases a sticky synthetic-left drag lock through M8's
//! aggregate-source rules. Scroll commits on centroid displacement from the
//! candidate centroid anchor (equality at the configured threshold commits);
//! the accepted accumulated displacement is emitted exactly once as
//! `ScrollDelta` when quantization yields a non-zero axis, then incremental
//! per-frame centroid deltas, each through the same per-axis sub-pixel
//! remainder invariant as the pointer (`ScrollDelta` values are typed
//! `LogicalPixels`; a zero/zero delta produces no event). Natural direction
//! is explicit: `natural=true` keeps the sign of the two-finger centroid
//! movement on each axis; `natural=false` negates each axis. `ScrollBegin`
//! is emitted exactly once per committed lifecycle and `ScrollEnd` exactly
//! once when the interaction ends (finger loss/gain, missing coordinates,
//! tracking replacement, discontinuity, deterministic cancellation, a
//! competing physical click, or `release_all`); no scroll event appears
//! before `ScrollBegin` or after `ScrollEnd`.
//!
//! A two-finger interaction is a secondary tap only when the policy is
//! enabled, both initial contacts were valid, no scroll committed, the
//! duration and each contact's **maximum displacement from its own anchor**
//! (not merely centroid motion, so opposing pinch/rotate-like motion can
//! never return and qualify) are within the limits, no third contact /
//! physical click / discontinuity / error competed, the interaction ends by
//! dropping below two fingers **with clean `Ended` evidence from at least one
//! anchored pair member** (a member that simply disappears cancels instead;
//! review M9 R6), and the continuing contact cluster is not tap-disqualified
//! by physical-button ownership (including a primary-left press begun before
//! the second finger), already-committed one-finger pointer ownership, or a
//! prior deterministic cancellation — the disqualification is **cluster
//! level** and survives until the cluster fully drains (review M9 R2/R3). It
//! emits exactly `ButtonDown(Right), ButtonUp(Right)` at most once, at the
//! first boundary that ends the exactly-two interaction. Scrolling is gated
//! by `TwoFingerConfig::scroll_enabled` on every commit/creation path: a
//! disabled scroll capability never opens or emits a scroll lifecycle (review
//! M9 R1). **Physical button ownership excludes scroll ownership as well as
//! secondary tap ownership** (review M9 R7): while aggregate physical Left or
//! Right is held (including a latched physical-left-as-right press), the
//! two-finger family neither anchors a candidate nor commits/emits
//! `ScrollBegin`/`ScrollDelta`, and a scroll cancelled by a physical press is
//! never re-opened on subsequent stable frames while that button remains held.
//! After the button is cleanly released, the same still-live pair may
//! establish a fresh relative scroll anchor (secondary tap stays
//! cluster-disqualified until the cluster drains). A physical-left press that
//! begins while exactly two complete valid
//! fingers are present (and the policy is enabled) is **latched** to the
//! secondary (right) button for the whole press: `ButtonDown(Right)` on
//! press, exactly one `ButtonUp(Right)` on the matching release, never
//! remapped by finger-count changes, and it cancels the secondary-tap/scroll
//! candidate with no synthetic tap on release. Same-frame output uses ordered
//! intents rather than global button bucketing: a pre-handoff left up
//! precedes a newly latched/physical right down, and an old-owner `ScrollEnd`
//! precedes the new physical-button down (review M9 R4); the
//! `physical_buttons.right` source is handled explicitly through the same
//! right-button multiplexer (never silently aliased with the latched or
//! synthetic sources). M9's boundary policy is: equality at the configured
//! duration/distance/threshold is accepted, strictly greater
//! expires/cancels, and timeouts are evaluated only at incoming frame
//! boundaries using [`ContactFrame::monotonic_timestamp`] and checked
//! `Duration` arithmetic. [`Arbiter::release_all`] deterministically emits
//! any required `ScrollEnd` and right/left button releases exactly once, then
//! resets every M7–M9 phase, anchor, remainder, disqualification flag, button
//! owner, and regression baseline; [`ArbiterSink::release_all`] reports every
//! failed explicit release structurally (`ReleaseFailed::primary` +
//! `ReleaseFailed::others`, review M9 R5) while preserving retry state and
//! the wrapped-cleanup error.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::contact::{Contact, ContactFrame, ContactState};
use crate::diagnostic::{Diagnostic, DiagnosticCode, DiagnosticLevel};
use crate::fidelity::{FidelityConfig, FidelityDeltaMm, FidelityOutcome, FidelityState};
use crate::gesture::{process_gesture, GestureConfig, GestureContact, GestureState};
use crate::gesture_bindings::{
    route_continuous_gesture, route_three_finger_tap, GestureMapConfig, GestureRouteState,
};
use crate::output::{MouseButton, OutputError, OutputEvent, OutputSink};
use crate::robustness::{
    filter_frame as robustness_filter_frame, ContactRole, RobustnessConfig, RobustnessState,
};
use crate::scroll_fidelity::{
    process_scroll, ScrollFidelityConfig, ScrollFidelityOutcome, ScrollFidelityState,
};
use crate::three_finger_drag::{
    process_three_finger_drag, ThreeFingerDragAction, ThreeFingerDragConfig, ThreeFingerDragState,
};
use crate::time::Monotonic;
use crate::units::{LogicalPixels, LogicalPixelsPerMm, Millimeters};

/// Lifecycle state of the arbiter's current interaction.
///
/// `Candidate`, `Committed`, `Cancelled`, and `Finished` are the four
/// observable interaction states required by M7; `Idle` is the arbiter's
/// no-interaction resting state (the origin of the machine).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Lifecycle {
    /// No interaction is active.
    Idle,
    /// A one-finger pointer/tap-family candidate; below its motion threshold
    /// and emitting no output.
    Candidate,
    /// The pointer interaction committed; relative movement is emitted.
    Committed,
    /// The last interaction was cancelled; no further output from it.
    Cancelled,
    /// The last interaction finished cleanly (its contact ended).
    Finished,
}

/// Observable tap/drag/drag-lock phase of the M8 policy.
///
/// The phase runs alongside the pointer [`Lifecycle`]: the lifecycle owns the
/// pointer interaction (`Candidate`/`Committed`/...), while the phase owns the
/// tap/tap-and-drag/sticky-drag-lock interpretation of that interaction. It
/// distinguishes at least: idle, a first-tap candidate, an open follow-up
/// window, an active tap-drag contact, locked-without-contact, a locked
/// continuation contact, and cancelled/finished outcomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TapDragPhase {
    /// No tap/drag/lock policy is active (also the resting phase when tapping
    /// is disabled by configuration).
    Idle,
    /// A one-finger contact is a first-tap candidate: tapping is enabled, the
    /// contact is below the tap movement limit so far, and no physical click
    /// has competed. It may still commit to pointer motion (which wins) or
    /// end as a qualifying tap.
    FirstTapCandidate,
    /// A qualifying first tap released and its synthetic-left press is held
    /// pending while the configured follow-up window is open. A new valid
    /// one-finger contact beginning at or before the deadline may reuse that
    /// press for tap-and-drag.
    FollowUpWindow,
    /// A follow-up one-finger contact is active while the first tap's
    /// synthetic-left press remains held. Pointer commitment turns that held
    /// press into a drag without another ButtonDown; a clean release resolves
    /// the multi-tap sequence.
    TapDragCandidate,
    /// Pointer motion on the follow-up contact has committed, so synthetic
    /// left is now held and the contact owns a real tap-and-drag.
    TapDragContact,
    /// Sticky drag lock: synthetic left is held with no live contact. A new
    /// contact may continue the drag; a qualifying tap releases the lock.
    LockedWithoutContact,
    /// A locked-contact continuation candidate/contact is active (synthetic
    /// left held, no new button down).
    LockedContact,
    /// The last tap/drag/lock interaction was cancelled (second live contact,
    /// discontinuity, missing coordinates, regression); no further tap/drag
    /// output is produced from it.
    Cancelled,
    /// The last tap/drag/lock interaction finished cleanly (non-qualifying
    /// end, drag ended without lock, second click completed, ...).
    Finished,
}

/// Observable two-finger phase of the M9 policy.
///
/// The phase runs alongside the pointer [`Lifecycle`] and the M8
/// [`TapDragPhase`] and owns the two-finger scroll / secondary-tap /
/// buttonpad physical-secondary-click interpretation. It distinguishes at
/// least: idle, a two-finger candidate, committed scrolling, a latched
/// physical-secondary click, and cancelled/finished outcomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TwoFingerPhase {
    /// No two-finger policy is active (also the resting phase when the M9
    /// configuration is absent).
    Idle,
    /// Exactly two complete live contacts form a two-finger candidate,
    /// anchored at the frame where the second valid contact appeared. **No
    /// pointer, button, or scroll event leaks during this period.** The
    /// candidate may commit to scrolling (which wins) or end as a qualifying
    /// secondary tap.
    Candidate,
    /// The two-finger scroll committed: `ScrollBegin` was emitted and
    /// relative `ScrollDelta`s are produced until the interaction ends
    /// (`ScrollEnd`).
    CommittedScroll,
    /// A buttonpad physical-left press while exactly two fingers were down
    /// was **latched** to the secondary (right) button: `ButtonDown(Right)`
    /// is held for the whole press and no scroll/tap output is produced
    /// until the matching physical release.
    PhysicalSecondaryClickHeld,
    /// The last two-finger interaction was cancelled (physical click, third
    /// finger, missing coordinates, tracking replacement, discontinuity,
    /// regression); no secondary tap and no further scroll output is
    /// produced from it.
    Cancelled,
    /// The last two-finger interaction finished cleanly (qualifying
    /// secondary tap fired, scroll ended by a clean release, or a latched
    /// physical click released).
    Finished,
}

/// One observable lifecycle transition produced while processing a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LifecycleTransition {
    /// A new one-finger candidate began for `tracking_id`.
    Begin { tracking_id: i32 },
    /// The candidate crossed its motion threshold and committed to pointer
    /// output for `tracking_id`.
    Commit { tracking_id: i32 },
    /// The active interaction for `tracking_id` was cancelled.
    Cancel { tracking_id: i32 },
    /// The active interaction for `tracking_id` finished cleanly.
    Finish { tracking_id: i32 },
}

/// The result of processing one frame: ordered events, lifecycle
/// transitions, the lifecycle after the frame, the tap/drag phase after the
/// frame, and arbiter diagnostics.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FrameDecision {
    /// Ordered semantic output events (press precedes drag movement, final
    /// movement precedes release; see module docs).
    pub events: Vec<OutputEvent>,
    /// The lifecycle transitions this frame produced (usually zero or one;
    /// a tracking-id replacement frame can carry `Finish` then `Begin`).
    pub transitions: Vec<LifecycleTransition>,
    /// The arbiter lifecycle after this frame.
    pub lifecycle_after: Lifecycle,
    /// The M8 tap/drag/drag-lock phase after this frame.
    pub tap_drag_phase_after: TapDragPhase,
    /// The M9 two-finger phase after this frame.
    pub two_finger_phase_after: TwoFingerPhase,
    /// Diagnostics attached by the arbiter (cancellation reasons,
    /// commit/finish notices, tap/drag/lock notices, two-finger
    /// scroll/tap/click notices, ignored active contacts).
    pub diagnostics: Vec<Diagnostic>,
}

/// Explicit M7 configuration: motion threshold and linear pointer scale.
///
/// Both values are validated at construction; there is deliberately no way to
/// build an invalid configuration (no serde `Deserialize`, private fields,
/// fallible constructor). The scale is validated by
/// [`LogicalPixelsPerMm::try_new`]; the threshold must be strictly positive.
///
/// M8: tapping is **disabled by default** — `ArbiterConfig::new` leaves the
/// tap configuration `None`, so all M7 below-threshold sequences remain
/// output-free. Attach a validated [`TapConfig`] with
/// [`ArbiterConfig::with_tap`] to enable tap-to-click, tap-and-drag, and/or
/// sticky drag lock.
#[derive(Clone, Debug, PartialEq)]
pub struct ArbiterConfig {
    motion_threshold_mm: Millimeters,
    logical_pixels_per_mm: LogicalPixelsPerMm,
    /// M8 tap/tap-and-drag/drag-lock configuration; `None` disables tapping.
    tap: Option<TapConfig>,
    /// M9 two-finger scroll / secondary-tap / physical-secondary-click
    /// configuration; `None` disables the two-finger family.
    two_finger: Option<TwoFingerConfig>,
    /// M11 experimental one-finger pointer-fidelity configuration; `None`
    /// (the default) executes the existing quantization branch unchanged and
    /// never passes committed pointer motion through M11 fidelity logic.
    fidelity: Option<FidelityConfig>,
    /// M12 experimental two-finger scroll-fidelity/momentum configuration;
    /// `None` preserves the M9 linear scroll path exactly.
    scroll_fidelity: Option<ScrollFidelityConfig>,
    /// M13 feature-aware contact robustness; `None` preserves pre-M13 input.
    robustness: Option<RobustnessConfig>,
    /// M14 continuous gesture recognizer; `None` preserves M13 and earlier
    /// ownership semantics exactly.
    gesture: Option<GestureConfig>,
    /// M15 three-finger drag/drag-lock policy; `None` preserves M14.
    three_finger_drag: Option<ThreeFingerDragConfig>,
    /// Optional pointer-fidelity profile used only while a committed
    /// three-finger drag owns pointer motion. `None` reuses the ordinary M11
    /// pointer-fidelity profile exactly.
    three_finger_drag_fidelity: Option<FidelityConfig>,
    /// M18 gesture-to-action routing; `None` preserves M17 exactly.
    gesture_bindings: Option<GestureMapConfig>,
}

impl ArbiterConfig {
    /// Creates a validated configuration with tapping **disabled** (M7
    /// behavior) and the M9 two-finger family **disabled**.
    ///
    /// Returns [`ArbiterConfigError::NonPositiveThreshold`] when
    /// `motion_threshold_mm` is not strictly positive (the scale is already
    /// validated by [`LogicalPixelsPerMm::try_new`]).
    pub fn new(
        motion_threshold_mm: Millimeters,
        logical_pixels_per_mm: LogicalPixelsPerMm,
    ) -> Result<Self, ArbiterConfigError> {
        if motion_threshold_mm.as_mm() <= 0.0 {
            return Err(ArbiterConfigError::NonPositiveThreshold(
                motion_threshold_mm,
            ));
        }
        Ok(Self {
            motion_threshold_mm,
            logical_pixels_per_mm,
            tap: None,
            two_finger: None,
            fidelity: None,
            scroll_fidelity: None,
            robustness: None,
            gesture: None,
            three_finger_drag: None,
            three_finger_drag_fidelity: None,
            gesture_bindings: None,
        })
    }

    /// Attaches a validated [`TapConfig`], enabling tapping exactly as its
    /// flags specify. The tap configuration is validated at its own
    /// construction ([`TapConfig::new`]), so this cannot fail.
    #[must_use]
    pub fn with_tap(mut self, tap: TapConfig) -> Self {
        self.tap = Some(tap);
        self
    }

    /// Attaches a validated [`TwoFingerConfig`], enabling the M9 two-finger
    /// family exactly as its flags specify. The configuration is validated
    /// at its own construction ([`TwoFingerConfig::new`]), so this cannot
    /// fail.
    #[must_use]
    pub fn with_two_finger(mut self, two_finger: TwoFingerConfig) -> Self {
        self.two_finger = Some(two_finger);
        self
    }

    /// Attaches a validated [`FidelityConfig`], enabling the experimental M11
    /// one-finger pointer-fidelity stage for committed pointer motion
    /// (M11_TASK.md §5/§6). The configuration is validated at its own
    /// construction ([`FidelityConfig::new`]), so this cannot fail. M10
    /// (`m10-linear-v1`) never attaches one, so its output path stays
    /// unchanged. **Batch 1**: the config field/exposure only — pointer
    /// routing through the Arbiter frame pipeline is a later batch.
    #[must_use]
    pub fn with_fidelity(mut self, fidelity: FidelityConfig) -> Self {
        self.fidelity = Some(fidelity);
        self
    }

    /// Attaches the validated M12 two-finger scroll-fidelity/momentum stage.
    #[must_use]
    pub fn with_scroll_fidelity(mut self, scroll_fidelity: ScrollFidelityConfig) -> Self {
        self.scroll_fidelity = Some(scroll_fidelity);
        self
    }

    #[must_use]
    pub fn with_robustness(mut self, robustness: RobustnessConfig) -> Self {
        self.robustness = Some(robustness);
        self
    }

    #[must_use]
    pub fn with_gesture(mut self, gesture: GestureConfig) -> Self {
        self.gesture = Some(gesture);
        self
    }

    #[must_use]
    pub fn with_three_finger_drag(mut self, drag: ThreeFingerDragConfig) -> Self {
        self.three_finger_drag = Some(drag);
        self
    }

    /// Overrides pointer fidelity only for committed three-finger drag
    /// movement. Other pointer interactions keep [`Self::fidelity_config`].
    #[must_use]
    pub fn with_three_finger_drag_fidelity(mut self, fidelity: FidelityConfig) -> Self {
        self.three_finger_drag_fidelity = Some(fidelity);
        self
    }

    #[must_use]
    pub fn with_gesture_bindings(mut self, bindings: GestureMapConfig) -> Self {
        self.gesture_bindings = Some(bindings);
        self
    }

    /// The distance a one-finger candidate must travel before committing.
    #[must_use]
    pub const fn motion_threshold_mm(&self) -> Millimeters {
        self.motion_threshold_mm
    }

    /// The linear millimetre-to-logical-pixel scale.
    #[must_use]
    pub const fn logical_pixels_per_mm(&self) -> LogicalPixelsPerMm {
        self.logical_pixels_per_mm
    }

    /// The attached tap/tap-and-drag/drag-lock configuration, when tapping is
    /// enabled at all. `None` preserves M7 behavior: no synthetic clicks and
    /// no tap policy.
    #[must_use]
    pub const fn tap_config(&self) -> Option<&TapConfig> {
        self.tap.as_ref()
    }

    /// Whether tap-to-click is enabled by this configuration.
    #[must_use]
    pub const fn is_tap_enabled(&self) -> bool {
        matches!(
            self.tap,
            Some(TapConfig {
                tap_enabled: true,
                ..
            })
        )
    }

    /// The attached M9 two-finger configuration, when the two-finger family
    /// is enabled at all. `None` preserves M7/M8 behavior: exactly-two live
    /// contacts simply cancel the one-finger interaction and no scroll /
    /// secondary-tap / physical-secondary-click output is produced.
    #[must_use]
    pub const fn two_finger_config(&self) -> Option<&TwoFingerConfig> {
        self.two_finger.as_ref()
    }

    /// Whether any M9 two-finger policy (scroll, secondary tap, or buttonpad
    /// physical secondary click) is enabled by this configuration.
    #[must_use]
    pub const fn is_two_finger_enabled(&self) -> bool {
        self.two_finger.is_some()
    }

    /// The attached experimental M11 fidelity configuration, when the M11
    /// one-finger pointer-fidelity stage is enabled at all (M11_TASK.md §5).
    /// `None` — the default, and always the case for `m10-linear-v1` —
    /// preserves the existing pointer quantization branch unchanged.
    #[must_use]
    pub const fn fidelity_config(&self) -> Option<&FidelityConfig> {
        self.fidelity.as_ref()
    }

    /// Whether the experimental M11 one-finger fidelity stage is enabled by
    /// this configuration.
    #[must_use]
    pub const fn is_fidelity_enabled(&self) -> bool {
        self.fidelity.is_some()
    }

    #[must_use]
    pub const fn scroll_fidelity_config(&self) -> Option<&ScrollFidelityConfig> {
        self.scroll_fidelity.as_ref()
    }

    #[must_use]
    pub const fn is_scroll_fidelity_enabled(&self) -> bool {
        self.scroll_fidelity.is_some()
    }

    #[must_use]
    pub const fn robustness_config(&self) -> Option<&RobustnessConfig> {
        self.robustness.as_ref()
    }

    #[must_use]
    pub const fn is_robustness_enabled(&self) -> bool {
        self.robustness.is_some()
    }

    #[must_use]
    pub const fn gesture_config(&self) -> Option<&GestureConfig> {
        self.gesture.as_ref()
    }

    #[must_use]
    pub const fn is_gesture_enabled(&self) -> bool {
        self.gesture.is_some()
    }

    #[must_use]
    pub const fn three_finger_drag_config(&self) -> Option<&ThreeFingerDragConfig> {
        self.three_finger_drag.as_ref()
    }

    #[must_use]
    pub const fn three_finger_drag_fidelity_config(&self) -> Option<&FidelityConfig> {
        self.three_finger_drag_fidelity.as_ref()
    }

    #[must_use]
    pub const fn is_three_finger_drag_enabled(&self) -> bool {
        self.three_finger_drag.is_some()
    }

    #[must_use]
    pub const fn gesture_bindings_config(&self) -> Option<&GestureMapConfig> {
        self.gesture_bindings.as_ref()
    }

    #[must_use]
    pub const fn is_gesture_bindings_enabled(&self) -> bool {
        self.gesture_bindings.is_some()
    }
}

/// Structured error from [`ArbiterConfig::new`].
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ArbiterConfigError {
    /// The motion threshold must be strictly positive.
    #[error("motion threshold must be strictly positive, got {0}")]
    NonPositiveThreshold(Millimeters),
}

/// Explicit M8 tap configuration: tap-to-click, tap-and-drag, and sticky
/// drag lock with their time/distance limits.
///
/// All values are validated at construction ([`TapConfig::new`]); there is
/// deliberately no way to build an invalid configuration. Validation rules:
///
/// * `max_tap_duration` and `max_tap_drag_gap` must be strictly positive
///   (zero/invalid durations are rejected, never silently coerced);
/// * `max_tap_movement_mm` must be strictly positive (it is always finite —
///   the [`Millimeters`] type rejects non-finite values at construction);
/// * feature combinations must be possible: tap-and-drag requires tap
///   enabled, and sticky drag lock requires tap-and-drag enabled.
///
/// Boundary policy (documented and tested): equality at the configured
/// duration/distance/gap is **accepted**; strictly greater **expires or
/// cancels** the tap/window. Timeouts are evaluated only at incoming frame
/// boundaries using `ContactFrame.monotonic_timestamp` and checked
/// `Duration` arithmetic; no wall clock or process-local clock is ever read.
///
/// The KDE/libinput values observed on the target machine are an A/B
/// reference only: this configuration is never read from KDE settings,
/// hidden defaults are never copied, and libinput is never consulted at
/// runtime.
#[derive(Clone, Debug, PartialEq)]
pub struct TapConfig {
    tap_enabled: bool,
    tap_and_drag_enabled: bool,
    drag_lock_enabled: bool,
    max_tap_duration: Duration,
    max_tap_movement_mm: Millimeters,
    max_tap_drag_gap: Duration,
}

impl TapConfig {
    /// Creates a validated tap configuration.
    ///
    /// # Errors
    ///
    /// Returns [`TapConfigError::ZeroDuration`] for a zero
    /// `max_tap_duration` or `max_tap_drag_gap`,
    /// [`TapConfigError::NonPositiveMovement`] for a non-positive
    /// `max_tap_movement_mm`, and
    /// [`TapConfigError::TapAndDragRequiresTap`] /
    /// [`TapConfigError::DragLockRequiresTapAndDrag`] for impossible feature
    /// combinations.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        tap_enabled: bool,
        tap_and_drag_enabled: bool,
        drag_lock_enabled: bool,
        max_tap_duration: Duration,
        max_tap_movement_mm: Millimeters,
        max_tap_drag_gap: Duration,
    ) -> Result<Self, TapConfigError> {
        if max_tap_duration.is_zero() {
            return Err(TapConfigError::ZeroDuration("max_tap_duration"));
        }
        if max_tap_drag_gap.is_zero() {
            return Err(TapConfigError::ZeroDuration("max_tap_drag_gap"));
        }
        if max_tap_movement_mm.as_mm() <= 0.0 {
            return Err(TapConfigError::NonPositiveMovement(max_tap_movement_mm));
        }
        if tap_and_drag_enabled && !tap_enabled {
            return Err(TapConfigError::TapAndDragRequiresTap);
        }
        if drag_lock_enabled && !tap_and_drag_enabled {
            return Err(TapConfigError::DragLockRequiresTapAndDrag);
        }
        Ok(Self {
            tap_enabled,
            tap_and_drag_enabled,
            drag_lock_enabled,
            max_tap_duration,
            max_tap_movement_mm,
            max_tap_drag_gap,
        })
    }

    /// Whether tap-to-click is enabled.
    #[must_use]
    pub const fn tap_enabled(&self) -> bool {
        self.tap_enabled
    }

    /// Whether a qualifying tap opens the follow-up window for tap-and-drag.
    #[must_use]
    pub const fn tap_and_drag_enabled(&self) -> bool {
        self.tap_and_drag_enabled
    }

    /// Whether sticky drag lock keeps synthetic left held after a tap-drag.
    #[must_use]
    pub const fn drag_lock_enabled(&self) -> bool {
        self.drag_lock_enabled
    }

    /// Returns the same validated tap policy with sticky drag lock disabled.
    ///
    /// Disabling drag lock can never violate the tap feature invariants: tap
    /// and tap-and-drag remain unchanged, while a committed tap-drag now emits
    /// its matching left-button release on the contact's clean Ended frame.
    #[must_use]
    pub fn without_drag_lock(mut self) -> Self {
        self.drag_lock_enabled = false;
        self
    }

    /// Returns the same tap policy with a different positive follow-up gap.
    ///
    /// Later profiles may refine interaction timing without rewriting the
    /// older profile's validated defaults.
    pub fn with_max_tap_drag_gap(mut self, gap: Duration) -> Result<Self, TapConfigError> {
        if gap.is_zero() {
            return Err(TapConfigError::ZeroDuration("max_tap_drag_gap"));
        }
        self.max_tap_drag_gap = gap;
        Ok(self)
    }

    /// Maximum duration of a one-finger contact that may still qualify as a
    /// tap. Equality is accepted; strictly longer cancels the tap.
    #[must_use]
    pub const fn max_tap_duration(&self) -> Duration {
        self.max_tap_duration
    }

    /// Maximum displacement of a first contact (from its own anchor) that may
    /// still qualify as a tap. Equality is accepted; strictly farther
    /// permanently makes the contact ineligible for tap, even if it returns
    /// to the anchor.
    #[must_use]
    pub const fn max_tap_movement_mm(&self) -> Millimeters {
        self.max_tap_movement_mm
    }

    /// Maximum gap from a completed tap to the next touch that may begin
    /// tap-and-drag. Equality is accepted; strictly greater closes the
    /// follow-up window (the next touch is an ordinary candidate).
    #[must_use]
    pub const fn max_tap_drag_gap(&self) -> Duration {
        self.max_tap_drag_gap
    }
}

/// Structured error from [`TapConfig::new`].
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum TapConfigError {
    /// A duration must be strictly positive; `0` is rejected, never coerced.
    #[error("tap configuration requires a strictly positive {0}")]
    ZeroDuration(&'static str),
    /// The maximum tap movement must be strictly positive.
    #[error("maximum tap movement must be strictly positive, got {0}")]
    NonPositiveMovement(Millimeters),
    /// Tap-and-drag is impossible without tap-to-click.
    #[error("tap-and-drag requires tap-to-click to be enabled")]
    TapAndDragRequiresTap,
    /// Sticky drag lock is impossible without tap-and-drag.
    #[error("sticky drag lock requires tap-and-drag to be enabled")]
    DragLockRequiresTapAndDrag,
}

/// Explicit M9 two-finger configuration: two-dimensional pixel scroll with an
/// explicit natural direction, two-finger secondary tap, and buttonpad
/// two-finger physical secondary click, with their time/distance limits.
///
/// All values are validated at construction ([`TwoFingerConfig::new`]); there
/// is deliberately no way to build an invalid configuration. Validation
/// rules:
///
/// * `scroll_commit_threshold_mm` must be strictly positive (always finite —
///   the [`Millimeters`] type rejects non-finite values at construction);
/// * `max_secondary_tap_duration` must be strictly positive (zero/invalid
///   durations are rejected, never silently coerced);
/// * `max_secondary_tap_movement_mm` must be strictly positive;
/// * the scroll scale is validated by [`LogicalPixelsPerMm::try_new`]
///   (finite and strictly positive).
///
/// Within M9's scope the three capabilities (scroll, secondary tap, and
/// buttonpad physical secondary click) are mutually independent, so no flag
/// combination is structurally impossible; every numeric limit is still
/// validated regardless of the enable flags (matching the M8 `TapConfig`
/// pattern — an unused limit is never silently coerced to a valid value).
///
/// Boundary policy (documented and tested): equality at the configured
/// duration/distance/scroll threshold is **accepted**; strictly greater
/// **expires or cancels**. Timeouts are evaluated only at incoming frame
/// boundaries using `ContactFrame.monotonic_timestamp` and checked
/// `Duration` arithmetic; no wall clock or process-local clock is ever read.
///
/// Natural direction is explicit: `natural=true` means the output scroll
/// delta has the same sign as the two-finger centroid movement on each axis
/// (content follows fingers); `natural=false` negates each axis. M10/M12 may
/// later calibrate the backend convention, but M9 never leaves the sign
/// implicit.
///
/// The KDE/libinput values observed on the target machine are an A/B
/// reference only: this configuration is never read from KDE settings,
/// hidden defaults are never copied, and libinput is never consulted at
/// runtime.
#[derive(Clone, Debug, PartialEq)]
pub struct TwoFingerConfig {
    scroll_enabled: bool,
    natural: bool,
    scroll_logical_pixels_per_mm: LogicalPixelsPerMm,
    scroll_commit_threshold_mm: Millimeters,
    secondary_tap_enabled: bool,
    two_finger_physical_click_enabled: bool,
    max_secondary_tap_duration: Duration,
    max_secondary_tap_movement_mm: Millimeters,
}

impl TwoFingerConfig {
    /// Creates a validated two-finger configuration.
    ///
    /// # Errors
    ///
    /// Returns [`TwoFingerConfigError::NonPositiveScrollThreshold`] for a
    /// non-positive `scroll_commit_threshold_mm`,
    /// [`TwoFingerConfigError::ZeroDuration`] for a zero
    /// `max_secondary_tap_duration`, and
    /// [`TwoFingerConfigError::NonPositiveMovement`] for a non-positive
    /// `max_secondary_tap_movement_mm`. The scroll scale is validated by
    /// [`LogicalPixelsPerMm::try_new`].
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        scroll_enabled: bool,
        natural: bool,
        scroll_logical_pixels_per_mm: LogicalPixelsPerMm,
        scroll_commit_threshold_mm: Millimeters,
        secondary_tap_enabled: bool,
        two_finger_physical_click_enabled: bool,
        max_secondary_tap_duration: Duration,
        max_secondary_tap_movement_mm: Millimeters,
    ) -> Result<Self, TwoFingerConfigError> {
        if scroll_commit_threshold_mm.as_mm() <= 0.0 {
            return Err(TwoFingerConfigError::NonPositiveScrollThreshold(
                scroll_commit_threshold_mm,
            ));
        }
        if max_secondary_tap_duration.is_zero() {
            return Err(TwoFingerConfigError::ZeroDuration(
                "max_secondary_tap_duration",
            ));
        }
        if max_secondary_tap_movement_mm.as_mm() <= 0.0 {
            return Err(TwoFingerConfigError::NonPositiveMovement(
                max_secondary_tap_movement_mm,
            ));
        }
        Ok(Self {
            scroll_enabled,
            natural,
            scroll_logical_pixels_per_mm,
            scroll_commit_threshold_mm,
            secondary_tap_enabled,
            two_finger_physical_click_enabled,
            max_secondary_tap_duration,
            max_secondary_tap_movement_mm,
        })
    }

    /// Whether two-finger pixel scrolling is enabled.
    #[must_use]
    pub const fn scroll_enabled(&self) -> bool {
        self.scroll_enabled
    }

    /// Whether the scroll direction is natural: `true` means the output
    /// scroll delta has the same sign as the two-finger centroid movement on
    /// each axis (content follows fingers); `false` negates each axis.
    #[must_use]
    pub const fn natural(&self) -> bool {
        self.natural
    }

    /// The linear millimetre-to-logical-pixel scale for scroll deltas.
    #[must_use]
    pub const fn scroll_logical_pixels_per_mm(&self) -> LogicalPixelsPerMm {
        self.scroll_logical_pixels_per_mm
    }

    /// The two-finger centroid displacement (from the candidate centroid
    /// anchor) that commits the scroll. Equality is accepted; strictly below
    /// remains a candidate.
    #[must_use]
    pub const fn scroll_commit_threshold_mm(&self) -> Millimeters {
        self.scroll_commit_threshold_mm
    }

    /// Whether a qualifying two-finger tap maps to one secondary (right)
    /// click pair.
    #[must_use]
    pub const fn secondary_tap_enabled(&self) -> bool {
        self.secondary_tap_enabled
    }

    /// Whether a buttonpad physical-left press while exactly two complete
    /// valid fingers are present is latched to the secondary (right) button
    /// for the whole press.
    #[must_use]
    pub const fn two_finger_physical_click_enabled(&self) -> bool {
        self.two_finger_physical_click_enabled
    }

    /// Maximum duration of a two-finger interaction that may still qualify as
    /// a secondary tap (measured from the anchoring frame). Equality is
    /// accepted; strictly longer disqualifies.
    #[must_use]
    pub const fn max_secondary_tap_duration(&self) -> Duration {
        self.max_secondary_tap_duration
    }

    /// Maximum per-contact displacement (each contact from its own anchor)
    /// that may still qualify as a secondary tap. Equality is accepted;
    /// strictly farther permanently disqualifies the interaction, even if the
    /// contacts return toward their anchors.
    #[must_use]
    pub const fn max_secondary_tap_movement_mm(&self) -> Millimeters {
        self.max_secondary_tap_movement_mm
    }
}

/// Structured error from [`TwoFingerConfig::new`].
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum TwoFingerConfigError {
    /// The scroll commit threshold must be strictly positive.
    #[error("scroll commit threshold must be strictly positive, got {0}")]
    NonPositiveScrollThreshold(Millimeters),
    /// A duration must be strictly positive; `0` is rejected, never coerced.
    #[error("two-finger configuration requires a strictly positive {0}")]
    ZeroDuration(&'static str),
    /// The maximum secondary-tap movement must be strictly positive.
    #[error("maximum secondary-tap movement must be strictly positive, got {0}")]
    NonPositiveMovement(Millimeters),
}

/// Structured error from [`Arbiter::validate_transition`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TransitionError {
    /// The lifecycle machine does not allow `from -> to` in one step.
    #[error("illegal lifecycle transition {from:?} -> {to:?}")]
    Illegal { from: Lifecycle, to: Lifecycle },
}

impl TransitionError {
    /// The lifecycle state the transition starts from.
    #[must_use]
    pub const fn from(&self) -> Lifecycle {
        match *self {
            TransitionError::Illegal { from, .. } => from,
        }
    }

    /// The lifecycle state the transition would have ended in.
    #[must_use]
    pub const fn to(&self) -> Lifecycle {
        match *self {
            TransitionError::Illegal { to, .. } => to,
        }
    }
}

/// Structured errors from [`Arbiter::frame`].
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ArbiterError {
    /// The frame failed model validation: [`ContactFrame::validate`] produced
    /// at least one `Error`/`Fatal` diagnostic (e.g. a negative live tracking
    /// id, non-finite pressure/orientation, a negative ellipse axis, or
    /// duplicate slots). The frame was rejected wholesale: no part of it was
    /// applied and no contact, button, scale, remainder, lifecycle, or
    /// regression-baseline state changed. Warning-only diagnostics (e.g. an
    /// incomplete `Began` contact) are not rejection triggers; the arbiter
    /// applies its own warning-only policy for those.
    #[error("frame {sequence} is invalid: {reason}")]
    InvalidFrame {
        sequence: u64,
        /// Stable codes of the `Error`/`Fatal` diagnostics that caused the
        /// rejection, in validation order.
        codes: Vec<DiagnosticCode>,
        /// Human-readable summary of the rejection cause.
        reason: String,
    },
    /// The frame sequence regressed; the active interaction (if any) was
    /// deterministically cancelled and no further pointer movement is emitted
    /// from it. The regression baseline is retained, so subsequent frames
    /// fail the same way until the arbiter is reset ([`Arbiter::release_all`]).
    #[error(
        "frame sequence regression: frame {found} must be greater than the previous frame {previous}"
    )]
    SequenceRegression { found: u64, previous: u64 },
    /// The frame timestamp regressed; the active interaction (if any) was
    /// deterministically cancelled (see [`ArbiterError::SequenceRegression`]).
    #[error("timestamp regression: frame timestamp {found:?} precedes the previous frame timestamp {previous:?}")]
    TimestampRegression {
        found: Monotonic,
        previous: Monotonic,
    },
    /// Motion arithmetic produced a non-finite result; the frame was rejected
    /// with no partial event batch and no state change (fail-closed).
    #[error("motion arithmetic produced a non-finite result while processing frame {sequence}")]
    NonFinite { sequence: u64 },
}

/// Structured errors from [`ArbiterSink`].
///
/// The adapter is **delivery-aware and fail-stop**: a sink rejection is never
/// collapsed into a bare sink error, and a partially delivered decision
/// faults the adapter until cleanup acknowledges every release.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ArbiterSinkError {
    /// The arbiter rejected the frame; its state is unchanged and the adapter
    /// is **not** faulted — a later valid frame may be fed normally.
    #[error("arbiter rejected the frame: {0}")]
    Arbiter(#[from] ArbiterError),
    /// The sink rejected one event of a decision mid-batch. Only
    /// `accepted_prefix` events were delivered; the adapter is now faulted and
    /// blocks further normal frames until [`ArbiterSink::release_all`]
    /// succeeds, because the output state has diverged from the decision
    /// state.
    #[error(
        "output sink rejected event {failed_event:?} at index {index} of {decision_len} (accepted prefix {accepted_prefix}): {primary}; the adapter is faulted and blocks frames until release_all succeeds"
    )]
    PartialSubmit {
        /// Index of the failed event within the decision's event list.
        index: usize,
        /// Number of events the sink accepted before the failure (equals
        /// `index`: the first rejection stops the batch).
        accepted_prefix: usize,
        /// Total number of events in the decision, so the caller can see how
        /// much was *not* delivered.
        decision_len: usize,
        /// The event the sink rejected.
        failed_event: OutputEvent,
        /// The primary sink failure.
        primary: OutputError,
    },
    /// Normal frames are blocked because a previous submission partially
    /// failed and the output state may diverge from the decision state. Call
    /// [`ArbiterSink::release_all`] to reset.
    #[error("arbiter sink is faulted after a partial output failure; call release_all to reset")]
    Faulted,
    /// Cleanup could not be fully acknowledged: the explicit release
    /// submission(s) and/or the wrapped sink's own cleanup
    /// ([`OutputSink::release_all`](crate::output::OutputSink::release_all))
    /// failed. The arbiter is *not* reset on this path; the caller should
    /// retry [`ArbiterSink::release_all`]. Held state is retained — and the
    /// next call re-submits the explicit up — only when the wrapped cleanup
    /// *also* failed. When the wrapped cleanup succeeded it is authoritative
    /// (it released all held state), so the release is reconciled as
    /// delivered and only the explicit-submission failures are reported here;
    /// a later recovery call does not re-submit an up.
    #[error(
        "output cleanup failed: release submission {primary:?}, additional release failures {others:?}, wrapped sink cleanup {cleanup:?}"
    )]
    ReleaseFailed {
        /// Failure of the first failed explicit release submission (in
        /// submission order: left up, then right up, then scroll end), when
        /// at least one failed.
        primary: Option<OutputError>,
        /// Additional explicit release submission failures beyond the first,
        /// in submission order — e.g. both `ButtonUp(Right)` and `ScrollEnd`
        /// failed (review M9 R5: every failed explicit release is observable,
        /// not collapsed to the first).
        others: Vec<OutputError>,
        /// Failure of the wrapped sink's own cleanup, when it failed.
        cleanup: Option<OutputError>,
    },
}

/// Collected output of one frame decision, built while processing a draft.
struct DraftOutput {
    events: Vec<OutputEvent>,
    transitions: Vec<LifecycleTransition>,
    diagnostics: Vec<Diagnostic>,
    /// Synthetic-left `false -> true` edge recorded by the M8 policy
    /// (tap pulse / tap-and-drag begin).
    synthetic_down: bool,
    /// Synthetic-left `true -> false` edge recorded by the M8 policy
    /// (tap pulse / drag end / fail-closed cancellation).
    synthetic_up: bool,
    /// Synthetic-right `false -> true` edge recorded by the M9 policy
    /// (two-finger secondary tap pulse).
    synthetic_right_down: bool,
    /// Synthetic-right `true -> false` edge recorded by the M9 policy
    /// (two-finger secondary tap pulse).
    synthetic_right_up: bool,
    /// Latched-right `false -> true` edge: a buttonpad physical-left press
    /// while exactly two fingers were present was latched to the secondary
    /// button (M9).
    latched_right_down: bool,
    /// Latched-right `true -> false` edge: the matching physical release of a
    /// latched secondary press (M9).
    latched_right_up: bool,
}

/// A two-dimensional quantity in millimetres, f64 precision (positions and
/// displacements used by the motion math).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Mm2 {
    x: f64,
    y: f64,
}

/// Per-frame pointer-routing inputs shared by every committed pointer delta:
/// the scale, the optional M11 fidelity config, the frame timestamp and
/// sequence, and the draft output sink.
///
/// Grouping these keeps the pointer-commit helpers ([`ArbiterState::commit`],
/// [`ArbiterState::commit_pointer`], and their callees) at small signatures
/// instead of a blanket `clippy::too_many_arguments` allow. The fidelity
/// config is `None` on every pre-M11 path, which keeps the existing
/// quantization branch byte-for-byte unchanged.
struct PointerRouting<'a> {
    ppm: f64,
    fidelity: Option<&'a FidelityConfig>,
    timestamp: Monotonic,
    sequence: u64,
    out: &'a mut DraftOutput,
}

/// Why a two-finger interaction ended.
enum TwoEnd {
    /// The interaction ended by dropping below exactly two fingers: a
    /// qualifying secondary tap may fire at most once.
    Release,
    /// The interaction was deterministically cancelled (third finger, missing
    /// coordinates, tracking-id replacement, discontinuity, physical click,
    /// regression): no secondary tap.
    Cancel(&'static str),
}

/// Sorts two tracking ids ascending, so the two-finger pair identity is
/// independent of slot/vector order.
const fn sorted_ids(a: i32, b: i32) -> (i32, i32) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

/// The arbiter's complete mutable state, cloned into an atomic frame draft.
///
/// [`Arbiter::frame`] computes the whole decision against a clone of this
/// state and commits it atomically only when every validation and every
/// arithmetic step has succeeded, so a rejected frame can never leave
/// half-applied contact, button, scale, remainder, lifecycle, tap, or timing
/// state.
///
/// Invariant: `tracking_id` is `Some` exactly when `lifecycle` is
/// `Candidate` or `Committed`.
#[derive(Clone, Debug, PartialEq)]
struct ArbiterState {
    lifecycle: Lifecycle,
    tracking_id: Option<i32>,
    /// Last consumed contact position in millimetres (f64 precision): the
    /// anchor while `Candidate`, the last emitted position while `Committed`.
    last_x_mm: Option<f64>,
    last_y_mm: Option<f64>,
    /// Per-axis unconsumed fractional pixels, each in `(-1, 1)`.
    remainder_x_px: f64,
    remainder_y_px: f64,
    /// Sequence of the last accepted frame (regression baseline).
    last_sequence: Option<u64>,
    /// Timestamp of the last accepted frame (regression baseline).
    last_timestamp: Option<Monotonic>,
    // --- M8 tap/tap-drag/drag-lock state ---
    /// The observable tap/drag phase (see [`TapDragPhase`]).
    tap_phase: TapDragPhase,
    /// Anchor of the current tap candidate (first-tap candidate or locked
    /// continuation contact), in millimetres.
    tap_anchor_x_mm: Option<f64>,
    tap_anchor_y_mm: Option<f64>,
    /// Maximum displacement of the tap candidate from its own anchor,
    /// tracked over every frame (not just the last delta). Crossing the tap
    /// movement limit permanently disqualifies the contact for tap, even if
    /// it returns to the anchor.
    tap_max_displacement_mm: f64,
    /// Timestamp of the tap candidate's `Began` frame.
    tap_began_timestamp: Option<Monotonic>,
    /// Timestamp of the last qualifying tap's release frame (opens the
    /// follow-up window).
    tap_completed_timestamp: Option<Monotonic>,
    /// Whether the tap-drag / locked continuation contact committed pointer
    /// motion (a "real drag"). Sticky drag lock only engages from a real
    /// drag; the fact survives the contact's lift into
    /// [`TapDragPhase::LockedWithoutContact`].
    drag_committed: bool,
    /// Whether a physical `ButtonDown(Left)` was emitted and not yet
    /// released. Driven by `ContactFrame.physical_buttons` edges; part of
    /// the M8 source-aware button model.
    physical_left: bool,
    /// Whether a synthetic `ButtonDown(Left)` (tap pulse / tap-and-drag /
    /// drag lock) is held. Part of the M8 source-aware button model; the
    /// output sink observes only the aggregate `physical_left || synthetic_left`.
    synthetic_left: bool,
    /// Whether the current one-finger contact began across a stream
    /// discontinuity (`discontinuity=true` on its `Began` frame). The
    /// runtime cannot know the real touch-down time or the movement before
    /// the recovered boundary, so the M8 tap family is ineligible for that
    /// contact: no first-tap candidate, no tap-and-drag press, no locked
    /// continuation, and no tap click on its release. M7 pointer
    /// re-anchoring is unaffected. Cleared when the interaction ends or is
    /// cancelled, so a later genuinely new `Began` may start tap policy
    /// normally (review M8 R3).
    tap_disqualified: bool,
    // --- M9 two-finger scroll / secondary tap / physical secondary click ---
    /// The observable two-finger phase (see [`TwoFingerPhase`]).
    two_phase: TwoFingerPhase,
    /// Tracking ids of the anchored two-finger pair, sorted ascending so the
    /// pair identity is independent of slot/vector order. `None` when no
    /// interaction is anchored.
    two_finger_ids: Option<(i32, i32)>,
    /// Anchor position (in millimetres, f64) of the lower-id contact.
    two_anchor_a: Option<(f64, f64)>,
    /// Anchor position of the higher-id contact.
    two_anchor_b: Option<(f64, f64)>,
    /// Current position of the lower-id contact.
    two_current_a: Option<(f64, f64)>,
    /// Current position of the higher-id contact.
    two_current_b: Option<(f64, f64)>,
    /// Centroid of the two anchors at the anchoring frame.
    two_centroid_anchor: Option<(f64, f64)>,
    /// Centroid of the two contacts as of the last processed frame.
    two_centroid_current: Option<(f64, f64)>,
    /// Maximum displacement of **each** contact from its own anchor (not
    /// merely centroid motion), so opposing pinch/rotate-like motion can
    /// never return and qualify as a secondary tap.
    two_max_displacement_mm: f64,
    /// Timestamp of the anchoring frame (measured with
    /// `ContactFrame.monotonic_timestamp`).
    two_began_timestamp: Option<Monotonic>,
    /// Per-axis unconsumed fractional scroll pixels, each in `(-1, 1)`.
    two_remainder_x_px: f64,
    two_remainder_y_px: f64,
    /// Whether the wire currently carries an open `ScrollBegin` that must be
    /// closed with `ScrollEnd`. `true` exactly while a committed scroll
    /// lifecycle is open; also the reconciled field for the
    /// accepted-prefix/fail-stop adapter.
    scroll_open: bool,
    /// Whether the **continuing contact cluster** is ineligible for a
    /// secondary tap (review M9 R2/R3). Set by: an anchored pair that began
    /// across a stream discontinuity, a physical button press competing in
    /// the cluster (latched or normal left, or physical right), an
    /// already-committed one-finger pointer interaction cancelled by the
    /// second finger, and any deterministic cancellation (third finger,
    /// missing coordinates, tracking replacement, regression,
    /// discontinuity). It is **cluster-level**: it survives interaction
    /// cancellation and re-anchoring, and is lifted only when the contact
    /// cluster fully drains (no live contacts) so a genuinely fresh cluster
    /// may be tap-eligible again. A `discontinuity=true` frame may still
    /// re-anchor a candidate for future relative scroll while disqualified.
    two_tap_disqualified: bool,
    /// Whether a buttonpad physical-left press was latched to the secondary
    /// (right) button for its whole duration. Set on the press edge while
    /// exactly two complete valid fingers are present (and the policy is
    /// enabled); cleared only on the matching physical-left release. While
    /// set, the press can never remap back to the left button.
    latched_right_owned: bool,
    /// Raw physical-left button state as reported by frames (independent of
    /// latching, so press/release edges are detectable while a latched press
    /// holds the physical button).
    physical_left_raw: bool,
    /// Physical-right button source state, driven by
    /// `ContactFrame.physical_buttons.right` edges; part of the M9
    /// right-button source model.
    physical_right: bool,
    /// Raw physical-right button state as reported by frames.
    physical_right_raw: bool,
    /// Synthetic-right source state (two-finger secondary tap pulse). Part
    /// of the M9 right-button source model; the output sink observes only
    /// the aggregate `physical_right || synthetic_right || latched_right_owned`.
    synthetic_right: bool,
    // --- M11 experimental one-finger pointer fidelity ---
    /// The M11 fidelity stage's runtime state (dead-zone accumulator `P`,
    /// velocity accumulators `V_pending`/`t_acc`, filtered velocity, and the
    /// stage's sample anchor). Stored **inside** `ArbiterState` so it
    /// participates in the existing copy/draft/commit behavior of
    /// [`Arbiter::frame`]: a rejected frame rolls it back with everything
    /// else. It is reset together with the one-finger interaction state
    /// (clean end, replacement, cancellation, discontinuity, `release_all`);
    /// the pixel remainder is **not** duplicated here — the existing
    /// `remainder_x_px`/`remainder_y_px` remain the only pointer remainder.
    /// Inert when `ArbiterConfig::fidelity_config()` is `None` (M10 path).
    fidelity: FidelityState,
    /// M12 scroll-fidelity / momentum state, stored in the atomic frame draft.
    scroll_fidelity: ScrollFidelityState,
    /// M13 tracking-id-sticky classifier, typing and jitter state.
    robustness: RobustnessState,
    /// M14 continuous gesture candidate/commit state.
    gesture: GestureState,
    /// M15 three-finger drag / lock state.
    three_finger_drag: ThreeFingerDragState,
    /// M18 single-fire/suppression state for mapped continuous gestures.
    gesture_route: GestureRouteState,
}

impl ArbiterState {
    fn fresh() -> Self {
        Self {
            lifecycle: Lifecycle::Idle,
            tracking_id: None,
            last_x_mm: None,
            last_y_mm: None,
            remainder_x_px: 0.0,
            remainder_y_px: 0.0,
            last_sequence: None,
            last_timestamp: None,
            tap_phase: TapDragPhase::Idle,
            tap_anchor_x_mm: None,
            tap_anchor_y_mm: None,
            tap_max_displacement_mm: 0.0,
            tap_began_timestamp: None,
            tap_completed_timestamp: None,
            drag_committed: false,
            physical_left: false,
            synthetic_left: false,
            tap_disqualified: false,
            two_phase: TwoFingerPhase::Idle,
            two_finger_ids: None,
            two_anchor_a: None,
            two_anchor_b: None,
            two_current_a: None,
            two_current_b: None,
            two_centroid_anchor: None,
            two_centroid_current: None,
            two_max_displacement_mm: 0.0,
            two_began_timestamp: None,
            two_remainder_x_px: 0.0,
            two_remainder_y_px: 0.0,
            scroll_open: false,
            two_tap_disqualified: false,
            latched_right_owned: false,
            physical_left_raw: false,
            physical_right: false,
            physical_right_raw: false,
            synthetic_right: false,
            fidelity: FidelityState::fresh(),
            scroll_fidelity: ScrollFidelityState::default(),
            robustness: RobustnessState::default(),
            gesture: GestureState::default(),
            three_finger_drag: ThreeFingerDragState::default(),
            gesture_route: GestureRouteState::default(),
        }
    }

    /// Whether an interaction is active (candidate or committed).
    fn has_interaction(&self) -> bool {
        matches!(self.lifecycle, Lifecycle::Candidate | Lifecycle::Committed)
    }

    /// Whether any **physical button ownership** is currently held — the
    /// aggregate physical Left source, the physical Right source, or a latched
    /// physical-left-as-right press (the raw physical button is down in every
    /// case). While any of these holds, the two-finger family must neither
    /// anchor a candidate nor commit/emit a scroll lifecycle (review M9 R7):
    /// a physical press deterministically cancels any two-finger interaction,
    /// and the continuing contact cluster must not re-open a scroll until the
    /// button is cleanly released (after release the same still-live pair may
    /// establish a fresh relative scroll anchor; secondary tap stays
    /// cluster-disqualified until the cluster drains).
    fn physical_button_ownership_held(&self) -> bool {
        self.physical_left || self.physical_right || self.latched_right_owned
    }

    /// Clears interaction state (tracking id, positions, remainder); leaves
    /// the lifecycle and button state untouched. Also clears the M8
    /// discontinuity disqualification, so a genuinely new contact after this
    /// interaction ends may start tap policy normally (review M8 R3).
    ///
    /// M11: the fidelity stage is reset together with the interaction — no
    /// timestamp, velocity, `P`, `V_pending`, or `t_acc` may leak into the
    /// next interaction (M11_TASK.md §9).
    fn clear_interaction(&mut self) {
        self.tracking_id = None;
        self.last_x_mm = None;
        self.last_y_mm = None;
        self.remainder_x_px = 0.0;
        self.remainder_y_px = 0.0;
        self.tap_disqualified = false;
        self.fidelity = FidelityState::fresh();
    }

    /// Clears the per-candidate tap tracking (anchor, maximum displacement,
    /// begin timestamp). The phase and the completed-tap timestamp (follow-up
    /// window) are managed separately.
    fn clear_tap_candidate(&mut self) {
        self.tap_anchor_x_mm = None;
        self.tap_anchor_y_mm = None;
        self.tap_max_displacement_mm = 0.0;
        self.tap_began_timestamp = None;
    }

    /// Clears the pending tap follow-up timestamp after cancellation, expiry,
    /// or drag commitment.
    fn clear_tap_chain(&mut self) {
        self.tap_completed_timestamp = None;
    }

    /// Updates the tap candidate's maximum displacement from its anchor with
    /// a newly observed position.
    fn update_tap_max_displacement(&mut self, x: f64, y: f64) {
        let (Some(ax), Some(ay)) = (self.tap_anchor_x_mm, self.tap_anchor_y_mm) else {
            return;
        };
        let d = ((x - ax) * (x - ax) + (y - ay) * (y - ay)).sqrt();
        if d > self.tap_max_displacement_mm {
            self.tap_max_displacement_mm = d;
        }
    }

    /// Whether the current tap candidate (first-tap candidate or locked
    /// continuation contact) qualifies as a tap at its release frame.
    ///
    /// Boundary policy: duration and displacement equality are accepted;
    /// strictly greater cancels. A physical press at any time during the
    /// candidate has already cancelled the candidate phase (see
    /// [`Arbiter::frame`]); this check additionally requires physical left
    /// not to be held at the release frame ("no physical click competed") and
    /// the contact not to have been seeded across a stream discontinuity
    /// (review M8 R3: the runtime cannot know its real touch-down time or
    /// pre-recovery movement).
    fn tap_candidate_qualifies(&self, frame_ts: Monotonic, config: &TapConfig) -> bool {
        let Some(began) = self.tap_began_timestamp else {
            return false;
        };
        let Some(duration) = frame_ts.duration_since(began) else {
            return false; // time went backwards: not a tap (regression handled upstream)
        };
        duration <= config.max_tap_duration()
            && self.tap_max_displacement_mm <= config.max_tap_movement_mm().as_mm() as f64
            && !self.physical_left
            && !self.tap_disqualified
    }

    /// Begins the synthetic left source: sets `synthetic_left`; the aggregate
    /// `ButtonDown(Left)` is recorded only if physical left is not already
    /// holding the wire ("no duplicate down").
    fn begin_synthetic(&mut self, out: &mut DraftOutput) {
        if !self.synthetic_left {
            self.synthetic_left = true;
            if !self.physical_left {
                out.synthetic_down = true;
            }
        }
    }

    /// Ends the synthetic left source: clears `synthetic_left`; the aggregate
    /// `ButtonUp(Left)` is recorded only if physical left is not still
    /// holding the wire ("ending the synthetic source must not emit up until
    /// physical left is released").
    fn end_synthetic(&mut self, out: &mut DraftOutput) {
        if self.synthetic_left {
            self.synthetic_left = false;
            if !self.physical_left {
                out.synthetic_up = true;
            }
        }
    }

    /// Cancels any active tap/drag/lock policy fail-open: ends the synthetic
    /// source (aggregate up unless physical holds), clears tap timing state,
    /// and reports the phase as [`TapDragPhase::Cancelled`]. No-op when no
    /// tap-family policy is active.
    fn cancel_tap_policy(&mut self, out: &mut DraftOutput) {
        match self.tap_phase {
            TapDragPhase::Idle | TapDragPhase::Cancelled | TapDragPhase::Finished => {}
            _ => {
                self.end_synthetic(out);
                self.tap_phase = TapDragPhase::Cancelled;
                self.clear_tap_candidate();
                self.clear_tap_chain();
                self.drag_committed = false;
            }
        }
    }

    /// Fail-closed cancellation for timestamp/sequence regression: the
    /// tap/drag/lock policy is cancelled but **any synthetic held state
    /// remains**, so the aggregate release stays observable to
    /// [`Arbiter::release_all`] (the unconditional escape path).
    fn fail_closed_cancel(&mut self) {
        if matches!(
            self.tap_phase,
            TapDragPhase::FirstTapCandidate
                | TapDragPhase::FollowUpWindow
                | TapDragPhase::TapDragCandidate
                | TapDragPhase::TapDragContact
                | TapDragPhase::LockedWithoutContact
                | TapDragPhase::LockedContact
        ) {
            self.tap_phase = TapDragPhase::Cancelled;
            self.clear_tap_candidate();
            self.clear_tap_chain();
            self.drag_committed = false;
        }
    }

    /// Begins the M8 tap-family interpretation for a new one-finger contact
    /// that is about to become a pointer candidate. Handles the follow-up
    /// window (tap-and-drag), the locked continuation, and the ordinary
    /// first-tap candidate.
    fn begin_tap_family(&mut self, pos: (f64, f64), frame_ts: Monotonic, config: &ArbiterConfig) {
        let (x, y) = pos;
        let tap = config.tap_config();
        if self.tap_disqualified {
            // The contact began across a stream discontinuity (review M8 R3):
            // the runtime cannot know its real touch-down time or the
            // movement before the recovered boundary, so the tap family is
            // ineligible for it — no first-tap candidate, no tap-and-drag
            // press from an open follow-up window, and no locked
            // continuation. The M7 pointer candidate still re-anchors below.
            self.tap_phase = TapDragPhase::Idle;
            self.clear_tap_chain();
            self.drag_committed = false;
            self.clear_tap_candidate();
            return;
        }
        match self.tap_phase {
            TapDragPhase::FollowUpWindow => {
                if tap.is_some_and(|t| t.tap_and_drag_enabled()) {
                    // Exactly one new valid finger began at or before the
                    // deadline. Do not press left yet: wait for an explicit
                    // pointer commit so a tracking-id bounce/re-touch cannot
                    // manufacture a held-left drag through another surface.
                    self.tap_phase = TapDragPhase::TapDragCandidate;
                    self.drag_committed = false;
                    self.tap_completed_timestamp = None;
                    self.tap_anchor_x_mm = Some(x);
                    self.tap_anchor_y_mm = Some(y);
                    self.tap_began_timestamp = Some(frame_ts);
                    self.tap_max_displacement_mm = 0.0;
                } else {
                    // Defensive: a window with tap-and-drag disabled (only
                    // reachable through stale state) closes without a press.
                    self.tap_phase = TapDragPhase::Idle;
                    self.clear_tap_chain();
                    self.clear_tap_candidate();
                }
            }
            TapDragPhase::LockedWithoutContact => {
                // One valid new finger begins a locked-contact continuation
                // without another button down.
                self.tap_phase = TapDragPhase::LockedContact;
                self.drag_committed = false;
                self.tap_anchor_x_mm = Some(x);
                self.tap_anchor_y_mm = Some(y);
                self.tap_began_timestamp = Some(frame_ts);
                self.tap_max_displacement_mm = 0.0;
            }
            _ => {
                if tap.is_some_and(|t| t.tap_enabled()) && !self.physical_left {
                    self.tap_completed_timestamp = None;
                    self.tap_phase = TapDragPhase::FirstTapCandidate;
                    self.tap_anchor_x_mm = Some(x);
                    self.tap_anchor_y_mm = Some(y);
                    self.tap_began_timestamp = Some(frame_ts);
                    self.tap_max_displacement_mm = 0.0;
                } else {
                    self.tap_phase = TapDragPhase::Idle;
                    self.clear_tap_candidate();
                }
            }
        }
    }

    /// Resolves the tap/drag/lock outcome at a contact's release frame (the
    /// `Ended` contact is `ended`, which may be absent). Runs before the M7
    /// interaction finishes.
    fn resolve_release_frame(
        &mut self,
        ended: Option<&Contact>,
        frame_ts: Monotonic,
        config: &ArbiterConfig,
        tracking_id: i32,
        sequence: u64,
        out: &mut DraftOutput,
    ) {
        // The release-frame position counts toward the tap's maximum
        // displacement (a tap's final position may be its farthest).
        if let Some(ended) = ended {
            if let Some((x, y)) = position(ended) {
                self.update_tap_max_displacement(x, y);
            }
        }
        let tap_cfg = config.tap_config();
        match self.tap_phase {
            TapDragPhase::FirstTapCandidate => {
                // A Began->Ended contact is a tap only when tapping is
                // enabled, required coordinates are valid (the Ended contact
                // carries them), the duration and maximum displacement are
                // within the limits, no extra live contact appeared, and no
                // physical click competed.
                let ended_complete = ended.is_some_and(Contact::is_complete);
                if ended_complete
                    && tap_cfg.is_some_and(|t| {
                        t.tap_enabled() && self.tap_candidate_qualifies(frame_ts, t)
                    })
                {
                    if tap_cfg.is_some_and(|t| t.tap_and_drag_enabled()) {
                        // libinput-style deferred commit: expose the press
                        // now but keep its release pending while this same
                        // interaction may still become a drag. Applications
                        // therefore cannot act on a completed click before
                        // the ambiguity is resolved.
                        self.begin_synthetic(out);
                        self.tap_completed_timestamp = Some(frame_ts);
                        self.tap_phase = TapDragPhase::FollowUpWindow;
                    } else {
                        // Tap-only mode has no drag ambiguity to defer.
                        self.begin_synthetic(out);
                        self.end_synthetic(out);
                        self.tap_phase = TapDragPhase::Finished;
                        self.clear_tap_chain();
                    }
                    out.diagnostics
                        .push(tap_fired_diagnostic(tracking_id, sequence));
                } else {
                    // Too long, too far, incomplete, cancelled, multi-contact,
                    // or discontinuous: no synthetic click.
                    self.tap_phase = TapDragPhase::Finished;
                    self.clear_tap_chain();
                }
                self.clear_tap_candidate();
            }
            TapDragPhase::TapDragCandidate => {
                // The first tap's synthetic left is still held. A clean
                // second tap closes that click and immediately starts the
                // next pending press, matching libinput's
                // DRAGGING_OR_DOUBLETAP transition.
                let ended_complete = ended.is_some_and(Contact::is_complete);
                if ended_complete
                    && tap_cfg.is_some_and(|t| {
                        t.tap_and_drag_enabled() && self.tap_candidate_qualifies(frame_ts, t)
                    })
                {
                    self.end_synthetic(out);
                    self.begin_synthetic(out);
                    out.diagnostics
                        .push(tap_fired_diagnostic(tracking_id, sequence));
                    self.tap_completed_timestamp = Some(frame_ts);
                    self.tap_phase = TapDragPhase::FollowUpWindow;
                } else {
                    self.end_synthetic(out);
                    self.tap_phase = TapDragPhase::Finished;
                    self.clear_tap_chain();
                }
                self.drag_committed = false;
                self.clear_tap_candidate();
            }
            TapDragPhase::TapDragContact => {
                // The committed follow-up drag ended. Final movement already
                // precedes this release; sticky drag lock may keep left held.
                if self.drag_committed {
                    if tap_cfg.is_some_and(|t| t.drag_lock_enabled()) {
                        self.tap_phase = TapDragPhase::LockedWithoutContact;
                        out.diagnostics
                            .push(drag_locked_diagnostic(tracking_id, sequence));
                    } else {
                        self.end_synthetic(out);
                        self.tap_phase = TapDragPhase::Finished;
                    }
                } else {
                    self.end_synthetic(out);
                    self.tap_phase = TapDragPhase::Finished;
                }
                self.drag_committed = false;
                self.clear_tap_candidate();
            }
            TapDragPhase::LockedContact => {
                if self.drag_committed {
                    // Committed drag: return to locked-without-contact on
                    // lift, still without an up.
                    self.tap_phase = TapDragPhase::LockedWithoutContact;
                } else if ended.is_some_and(Contact::is_complete)
                    && tap_cfg.is_some_and(|t| {
                        t.tap_enabled() && self.tap_candidate_qualifies(frame_ts, t)
                    })
                {
                    // Qualifying tap without committed movement: emit exactly
                    // one logical left up and leave drag lock.
                    self.end_synthetic(out);
                    self.tap_phase = TapDragPhase::Idle;
                    out.diagnostics
                        .push(drag_unlocked_diagnostic(tracking_id, sequence));
                } else {
                    // Non-qualifying long/too-far contact that never commits
                    // motion: no fabricated click; the lock stays held for
                    // another continuation attempt.
                    self.tap_phase = TapDragPhase::LockedWithoutContact;
                }
                self.drag_committed = false;
                self.clear_tap_candidate();
            }
            _ => {
                // Plain pointer interaction (or tap disabled): nothing to
                // resolve.
            }
        }
    }

    /// Resolves the tap/drag/lock outcome when the old interaction's contact
    /// is replaced by a new tracking id (before the M7 finish/begin pair).
    fn resolve_replacement_old(
        &mut self,
        config: &ArbiterConfig,
        tracking_id: i32,
        sequence: u64,
        out: &mut DraftOutput,
    ) {
        let tap_cfg = config.tap_config();
        match self.tap_phase {
            TapDragPhase::FirstTapCandidate => {
                // A tracking-id replacement is not a clean Began->Ended tap:
                // no synthetic click.
                self.tap_phase = TapDragPhase::Finished;
                self.clear_tap_candidate();
                self.clear_tap_chain();
            }
            TapDragPhase::TapDragCandidate => {
                // A replacement inside the follow-up contact is not a clean
                // second click. The first tap's deferred press is already
                // held, so replacement must release it; in particular, never
                // carry held-left ownership through a tracking-id bounce.
                self.end_synthetic(out);
                self.tap_phase = TapDragPhase::Finished;
                self.drag_committed = false;
                self.clear_tap_candidate();
                self.clear_tap_chain();
            }
            TapDragPhase::TapDragContact => {
                if self.drag_committed && tap_cfg.is_some_and(|t| t.drag_lock_enabled()) {
                    self.tap_phase = TapDragPhase::LockedWithoutContact;
                    out.diagnostics
                        .push(drag_locked_diagnostic(tracking_id, sequence));
                } else {
                    self.end_synthetic(out);
                    self.tap_phase = TapDragPhase::Finished;
                }
                self.drag_committed = false;
                self.clear_tap_candidate();
                self.clear_tap_chain();
            }
            TapDragPhase::LockedContact => {
                // A replaced locked contact never produced a clean tap; the
                // lock stays held for another continuation attempt.
                self.tap_phase = TapDragPhase::LockedWithoutContact;
                self.drag_committed = false;
                self.clear_tap_candidate();
            }
            _ => {}
        }
    }

    /// Records the effect of pointer commitment on the tap/drag policy:
    /// pointer commitment wins over tap — a committed contact is a real drag
    /// and can never also produce a tap click.
    fn note_pointer_commit(&mut self) {
        match self.tap_phase {
            TapDragPhase::FirstTapCandidate => {
                self.tap_phase = TapDragPhase::Idle;
                self.clear_tap_candidate();
                self.clear_tap_chain();
            }
            TapDragPhase::TapDragContact | TapDragPhase::LockedContact => {
                self.drag_committed = true;
            }
            _ => {}
        }
    }

    /// Converts a pending tap-and-drag follow-up contact into a real held
    /// synthetic-left drag immediately before the first committed pointer
    /// delta is emitted.
    fn prepare_pointer_commit(
        &mut self,
        _tap_cfg: Option<&TapConfig>,
        tracking_id: i32,
        sequence: u64,
        out: &mut DraftOutput,
    ) {
        if self.tap_phase == TapDragPhase::TapDragCandidate {
            // The first tap already owns synthetic left. Motion merely
            // resolves the pending press as a drag; no second ButtonDown is
            // generated.
            self.tap_phase = TapDragPhase::TapDragContact;
            self.drag_committed = true;
            self.clear_tap_candidate();
            self.clear_tap_chain();
            out.diagnostics
                .push(tap_and_drag_began_diagnostic(tracking_id, sequence));
        }
    }

    // ------------------------------------------------------------------
    // M9: two-finger scroll / secondary tap / physical secondary click
    // ------------------------------------------------------------------

    /// Begins the synthetic-right source: sets `synthetic_right`; the
    /// aggregate `ButtonDown(Right)` is recorded only if neither the physical
    /// right source nor a latched secondary press already holds the wire
    /// ("no duplicate down").
    fn begin_synthetic_right(&mut self, out: &mut DraftOutput) {
        if !self.synthetic_right {
            self.synthetic_right = true;
            if !self.physical_right && !self.latched_right_owned {
                out.synthetic_right_down = true;
            }
        }
    }

    /// Ends the synthetic-right source: clears `synthetic_right`; the
    /// aggregate `ButtonUp(Right)` is recorded only if no other right source
    /// still holds the wire ("no up until the physical/latched sources
    /// release").
    fn end_synthetic_right(&mut self, out: &mut DraftOutput) {
        if self.synthetic_right {
            self.synthetic_right = false;
            if !self.physical_right && !self.latched_right_owned {
                out.synthetic_right_up = true;
            }
        }
    }

    /// Clears the per-interaction two-finger state (pair identity, anchors,
    /// currents, centroid, maximum displacement, begin timestamp, and
    /// remainders). The phase, the `scroll_open` wire flag, the latched-right
    /// ownership, and the **cluster-level** `two_tap_disqualified` flag are
    /// managed separately: the disqualification survives interaction
    /// cancellation (third finger, missing coordinates, tracking replacement,
    /// discontinuity, physical click, regression) and is lifted only when the
    /// contact cluster fully drains — a genuinely fresh cluster may be
    /// tap-eligible again (review M9 R3).
    fn clear_two_finger_interaction(&mut self) {
        self.two_finger_ids = None;
        self.two_anchor_a = None;
        self.two_anchor_b = None;
        self.two_current_a = None;
        self.two_current_b = None;
        self.two_centroid_anchor = None;
        self.two_centroid_current = None;
        self.two_max_displacement_mm = 0.0;
        self.two_began_timestamp = None;
        self.two_remainder_x_px = 0.0;
        self.two_remainder_y_px = 0.0;
        self.scroll_fidelity.reset();
    }

    /// Updates the two-finger maximum per-contact displacement with a newly
    /// observed position of a contact that belongs to the anchored pair
    /// (identified by tracking id, compared against its **own** anchor — not
    /// merely centroid motion, so opposing pinch/rotate-like motion can never
    /// return and qualify as a secondary tap).
    fn update_two_max_displacement_for(&mut self, tracking_id: i32, pos: (f64, f64)) {
        let (Some(ids), Some(anchor_a), Some(anchor_b)) =
            (self.two_finger_ids, self.two_anchor_a, self.two_anchor_b)
        else {
            return;
        };
        let anchor = if tracking_id == ids.0 {
            anchor_a
        } else if tracking_id == ids.1 {
            anchor_b
        } else {
            return;
        };
        let d = ((pos.0 - anchor.0) * (pos.0 - anchor.0) + (pos.1 - anchor.1) * (pos.1 - anchor.1))
            .sqrt();
        if d > self.two_max_displacement_mm {
            self.two_max_displacement_mm = d;
        }
    }

    /// Updates the two-finger maximum per-contact displacement from every
    /// positioned contact of the anchored pair present in the frame (used at
    /// the release boundary, where an `Ended` contact carries its final
    /// position — the final position may be the contact's farthest).
    fn update_two_max_displacement_from_frame(&mut self, frame: &ContactFrame) {
        for contact in &frame.contacts {
            if let Some(pos) = position(contact) {
                self.update_two_max_displacement_for(contact.tracking_id, pos);
            }
        }
    }

    /// Begins a two-finger candidate anchored at this frame: the frame where
    /// the second valid contact appears anchors the interaction, its centroid
    /// anchors the scroll, and each contact's own anchor tracks its maximum
    /// displacement. Precondition: `id_a != id_b` and both positions are known
    /// (the caller filters complete live contacts). A candidate anchored
    /// across a stream discontinuity is ineligible for secondary tap.
    #[allow(clippy::too_many_arguments)]
    fn begin_two_finger_candidate(
        &mut self,
        id_a: i32,
        pos_a: (f64, f64),
        id_b: i32,
        pos_b: (f64, f64),
        frame_ts: Monotonic,
        sequence: u64,
        discontinuity: bool,
        out: &mut DraftOutput,
    ) {
        // Pair identity is independent of slot/vector order: sort the ids and
        // attribute each position to its own sorted id.
        let (lo, hi) = sorted_ids(id_a, id_b);
        let (a1, a2) = if id_a == lo {
            (pos_a, pos_b)
        } else {
            (pos_b, pos_a)
        };
        let centroid = ((a1.0 + a2.0) / 2.0, (a1.1 + a2.1) / 2.0);
        self.two_phase = TwoFingerPhase::Candidate;
        self.two_finger_ids = Some((lo, hi));
        self.two_anchor_a = Some(a1);
        self.two_anchor_b = Some(a2);
        self.two_current_a = Some(a1);
        self.two_current_b = Some(a2);
        self.two_centroid_anchor = Some(centroid);
        self.two_centroid_current = Some(centroid);
        self.two_max_displacement_mm = 0.0;
        self.two_began_timestamp = Some(frame_ts);
        self.two_remainder_x_px = 0.0;
        self.two_remainder_y_px = 0.0;
        self.scroll_fidelity.reset();
        // The continuing contact cluster is ineligible for secondary tap when
        // any competing ownership is already established at the anchoring
        // boundary, and that disqualification persists for the cluster (OR,
        // never overwrite):
        //  * contacts seeded across a recovered boundary have unknown real
        //    down time and prior movement (discontinuity);
        //  * a physical left or right press already holds the button (review
        //    M9 R2: a press begun before the second finger appeared is a
        //    primary-left ownership of the same cluster);
        //  * a prior cancellation (third finger, missing coordinates,
        //    tracking replacement, regression, physical click) already
        //    disqualified the cluster (review M9 R3).
        self.two_tap_disqualified = self.two_tap_disqualified
            || discontinuity
            || self.physical_left
            || self.physical_right
            || self.latched_right_owned;
        out.diagnostics
            .push(two_finger_scroll_began_diagnostic(sequence, lo, hi));
    }

    /// Updates the anchored pair's positions, per-contact maximum
    /// displacement, and centroid, then commits or continues the scroll.
    #[allow(clippy::too_many_arguments)]
    fn update_two_finger_pair(
        &mut self,
        id1: i32,
        p1: (f64, f64),
        id2: i32,
        p2: (f64, f64),
        frame: &ContactFrame,
        two_cfg: &TwoFingerConfig,
        scroll_cfg: Option<&ScrollFidelityConfig>,
        out: &mut DraftOutput,
    ) -> Result<(), ArbiterError> {
        let sequence = frame.sequence;
        // Attribute each position to its own anchor (the pair is sorted).
        let (a1, a2) = if id1 <= id2 { (p1, p2) } else { (p2, p1) };
        if let (Some(anchor_a), Some(anchor_b)) = (self.two_anchor_a, self.two_anchor_b) {
            let d1 = ((a1.0 - anchor_a.0) * (a1.0 - anchor_a.0)
                + (a1.1 - anchor_a.1) * (a1.1 - anchor_a.1))
                .sqrt();
            let d2 = ((a2.0 - anchor_b.0) * (a2.0 - anchor_b.0)
                + (a2.1 - anchor_b.1) * (a2.1 - anchor_b.1))
                .sqrt();
            let d = d1.max(d2);
            if d > self.two_max_displacement_mm {
                self.two_max_displacement_mm = d;
            }
        }
        self.two_current_a = Some(a1);
        self.two_current_b = Some(a2);
        let centroid = ((a1.0 + a2.0) / 2.0, (a1.1 + a2.1) / 2.0);
        match self.two_phase {
            TwoFingerPhase::Candidate => {
                if !two_cfg.scroll_enabled() || self.physical_button_ownership_held() {
                    // Scrolling is explicitly disabled (review M9 R1) or a
                    // physical button ownership is held (review M9 R7): the
                    // candidate must never open or emit a scroll lifecycle,
                    // regardless of how far the centroid moves. In the
                    // button-held case this branch is defensive — candidate
                    // anchoring is itself gated on no button ownership — but
                    // it makes the exclusive-ownership invariant explicit: no
                    // frame can commit a scroll while a physical button is
                    // held. The candidate remains a candidate so a qualifying
                    // secondary tap may still fire (button-held candidates are
                    // cluster-disqualified, so in practice nothing fires);
                    // only the per-contact displacement tracking above and the
                    // centroid update below apply.
                } else {
                    // Scroll commits on centroid displacement from the
                    // candidate centroid anchor; equality at the threshold
                    // commits.
                    let Some(anchor) = self.two_centroid_anchor else {
                        return Ok(());
                    };
                    let acc = (centroid.0 - anchor.0, centroid.1 - anchor.1);
                    let magnitude = (acc.0 * acc.0 + acc.1 * acc.1).sqrt();
                    if magnitude >= two_cfg.scroll_commit_threshold_mm().as_mm() as f64 {
                        // Commit: ScrollBegin, then the accepted accumulated
                        // centroid displacement exactly once as ScrollDelta
                        // when quantization yields a non-zero axis.
                        self.two_phase = TwoFingerPhase::CommittedScroll;
                        out.events.push(OutputEvent::ScrollBegin);
                        self.scroll_open = true;
                        out.diagnostics
                            .push(two_finger_scroll_committed_diagnostic(sequence));
                        self.emit_scroll_delta(acc, frame, two_cfg, scroll_cfg, out)?;
                    }
                }
            }
            TwoFingerPhase::CommittedScroll => {
                // Incremental per-frame centroid deltas through the same
                // per-axis sub-pixel remainder invariant.
                let Some(prev) = self.two_centroid_current else {
                    return Ok(());
                };
                let delta = (centroid.0 - prev.0, centroid.1 - prev.1);
                self.emit_scroll_delta(delta, frame, two_cfg, scroll_cfg, out)?;
            }
            _ => {}
        }
        self.two_centroid_current = Some(centroid);
        Ok(())
    }

    fn emit_scroll_delta(
        &mut self,
        delta_mm: (f64, f64),
        frame: &ContactFrame,
        two_cfg: &TwoFingerConfig,
        scroll_cfg: Option<&ScrollFidelityConfig>,
        out: &mut DraftOutput,
    ) -> Result<(), ArbiterError> {
        let sequence = frame.sequence;
        let (scaled_x, scaled_y) = if let Some(scroll_cfg) = scroll_cfg {
            match process_scroll(
                scroll_cfg,
                &mut self.scroll_fidelity,
                delta_mm,
                frame.monotonic_timestamp,
                two_cfg.scroll_logical_pixels_per_mm(),
                two_cfg.natural(),
            )
            .map_err(|_| ArbiterError::NonFinite { sequence })?
            {
                ScrollFidelityOutcome::Hold => return Ok(()),
                ScrollFidelityOutcome::EmitScaledPixels { x, y, .. } => (x, y),
            }
        } else {
            let ppm = f64::from(two_cfg.scroll_logical_pixels_per_mm().as_px_per_mm());
            let sign = if two_cfg.natural() { 1.0 } else { -1.0 };
            (delta_mm.0 * ppm * sign, delta_mm.1 * ppm * sign)
        };

        let (emitted_x, remainder_x) = quantize(scaled_x, self.two_remainder_x_px);
        let (emitted_y, remainder_y) = quantize(scaled_y, self.two_remainder_y_px);
        self.two_remainder_x_px = remainder_x;
        self.two_remainder_y_px = remainder_y;
        push_scroll_delta(&mut out.events, sequence, emitted_x, emitted_y)
    }

    /// Whether the anchored two-finger candidate qualifies as a secondary tap
    /// at its release boundary. Boundary policy: duration and per-contact
    /// displacement equality are accepted; strictly greater disqualifies. A
    /// physical click at any time has already cancelled the candidate (see
    /// [`Arbiter::frame`]); this check additionally requires that no physical
    /// left or right source holds at the release boundary and that the pair
    /// did not begin across a stream discontinuity. Physical button ownership
    /// — including a physical-left press that began before the second finger
    /// appeared — permanently disqualifies the continuing contact cluster
    /// (review M9 R2); the anchor-time check in
    /// [`begin_two_finger_candidate`](ArbiterState::begin_two_finger_candidate)
    /// records that disqualification, and this release-boundary check is the
    /// defensive second gate.
    fn secondary_tap_qualifies(&self, frame_ts: Monotonic, config: &ArbiterConfig) -> bool {
        let Some(two_cfg) = config.two_finger_config() else {
            return false;
        };
        if !two_cfg.secondary_tap_enabled() {
            return false;
        }
        if self.two_tap_disqualified {
            return false;
        }
        let Some(began) = self.two_began_timestamp else {
            return false;
        };
        let Some(duration) = frame_ts.duration_since(began) else {
            return false; // time went backwards: not a tap (regression handled upstream)
        };
        duration <= two_cfg.max_secondary_tap_duration()
            && self.two_max_displacement_mm
                <= two_cfg.max_secondary_tap_movement_mm().as_mm() as f64
            && !self.physical_left
            && !self.physical_right
            && !self.latched_right_owned
    }

    /// Ends the active two-finger interaction. On a clean release with phase
    /// `Candidate`, a qualifying secondary tap fires its right click pair at
    /// most once (at the first boundary that ends the exactly-two
    /// interaction). A committed scroll emits `ScrollEnd` exactly once before
    /// leaving the scroll phase. Per-interaction anchors/remainders are
    /// cleared. The latched physical press is owned by the physical release
    /// path, never by this method.
    fn end_two_finger(
        &mut self,
        end: TwoEnd,
        frame: &ContactFrame,
        config: &ArbiterConfig,
        out: &mut DraftOutput,
    ) -> Result<(), ArbiterError> {
        let sequence = frame.sequence;
        let frame_ts = frame.monotonic_timestamp;
        match self.two_phase {
            TwoFingerPhase::Candidate => match end {
                TwoEnd::Release => {
                    // The release-frame positions count toward the maximum
                    // per-contact displacement (a final position may be the
                    // farthest).
                    self.update_two_max_displacement_from_frame(frame);
                    if self.secondary_tap_qualifies(frame_ts, config) {
                        // Exactly ButtonDown(Right), ButtonUp(Right) in order
                        // at the qualifying release boundary; no interleaved
                        // pointer/scroll output.
                        self.begin_synthetic_right(out);
                        self.end_synthetic_right(out);
                        out.diagnostics
                            .push(secondary_tap_fired_diagnostic(sequence));
                    }
                    self.two_phase = TwoFingerPhase::Finished;
                }
                TwoEnd::Cancel(reason) => {
                    self.two_phase = TwoFingerPhase::Cancelled;
                    // The deterministic cancellation itself disqualifies the
                    // continuing contact cluster for secondary tap (third
                    // finger, missing coordinates, tracking replacement, ...):
                    // the disqualification survives until the cluster drains
                    // (review M9 R3).
                    self.two_tap_disqualified = true;
                    out.diagnostics
                        .push(two_finger_cancelled_diagnostic(reason, sequence));
                }
            },
            TwoFingerPhase::CommittedScroll => {
                match end {
                    TwoEnd::Release => {
                        self.two_phase = TwoFingerPhase::Finished;
                        // libinput boundary: the input layer ends finger
                        // scrolling when the fingers end. Kinetic continuation
                        // belongs to the scroll consumer/toolkit, which knows
                        // the target widget and can cancel inertia when that
                        // context changes. Core therefore never fabricates
                        // post-contact ScrollDelta events.
                        if self.scroll_open {
                            out.events.push(OutputEvent::ScrollEnd);
                            self.scroll_open = false;
                            out.diagnostics
                                .push(two_finger_scroll_ended_diagnostic(sequence));
                        }
                    }
                    TwoEnd::Cancel(reason) => {
                        if self.scroll_open {
                            out.events.push(OutputEvent::ScrollEnd);
                            self.scroll_open = false;
                            out.diagnostics
                                .push(two_finger_scroll_ended_diagnostic(sequence));
                        }
                        self.two_phase = TwoFingerPhase::Cancelled;
                        // The cancellation disqualifies the continuing
                        // contacts for secondary tap (review M9 R3).
                        self.two_tap_disqualified = true;
                        out.diagnostics
                            .push(two_finger_cancelled_diagnostic(reason, sequence));
                    }
                }
            }
            _ => {}
        }
        self.clear_two_finger_interaction();
        Ok(())
    }

    /// A buttonpad physical-left press while exactly two complete valid
    /// fingers are present was latched to the secondary (right) button: a
    /// two-finger candidate is cancelled (no synthetic secondary tap on
    /// release) and a committed scroll ends (`ScrollEnd`) because the
    /// physical secondary click owns the interaction and it cannot commit
    /// scroll/tap output. The phase becomes
    /// [`TwoFingerPhase::PhysicalSecondaryClickHeld`] and stays there until
    /// the matching physical release.
    fn latch_secondary_press(&mut self, out: &mut DraftOutput, sequence: u64) {
        if self.two_phase == TwoFingerPhase::CommittedScroll && self.scroll_open {
            out.events.push(OutputEvent::ScrollEnd);
            self.scroll_open = false;
            out.diagnostics
                .push(two_finger_scroll_ended_diagnostic(sequence));
        }
        if matches!(
            self.two_phase,
            TwoFingerPhase::Candidate | TwoFingerPhase::CommittedScroll
        ) {
            self.clear_two_finger_interaction();
        }
        // The continuing contacts are tap-disqualified: a physical click
        // competed (whether a candidate was active or the click began on the
        // very first two-finger frame), so they must not re-seed a
        // tap-eligible candidate after the press ends — they may re-anchor
        // for relative scroll only after the press is cleanly released
        // (review M9 R7: physical button ownership excludes scroll ownership
        // while held).
        self.two_tap_disqualified = true;
        self.two_phase = TwoFingerPhase::PhysicalSecondaryClickHeld;
        out.diagnostics
            .push(secondary_click_latched_diagnostic(sequence));
    }

    /// A non-latched physical press (normal left, or physical right)
    /// competes with a two-finger candidate/scroll: the candidate is
    /// cancelled (no secondary tap) and a committed scroll ends (`ScrollEnd`).
    /// The continuing contacts are also tap-disqualified: a physical click
    /// competed, so the same fingers must not immediately re-seed a
    /// tap-eligible candidate (their interaction with the click is unknown).
    /// Scroll is likewise excluded while the press is held (review M9 R7):
    /// candidate anchoring and scroll commit are both gated on no physical
    /// button ownership, so the continuing contacts may re-anchor for
    /// relative scroll only after the button is cleanly released.
    fn cancel_two_finger_for_physical_press(&mut self, out: &mut DraftOutput, sequence: u64) {
        match self.two_phase {
            TwoFingerPhase::Candidate => {
                self.two_phase = TwoFingerPhase::Cancelled;
                self.clear_two_finger_interaction();
                self.two_tap_disqualified = true;
                out.diagnostics
                    .push(two_finger_cancelled_diagnostic("physical click", sequence));
            }
            TwoFingerPhase::CommittedScroll => {
                if self.scroll_open {
                    out.events.push(OutputEvent::ScrollEnd);
                    self.scroll_open = false;
                    out.diagnostics
                        .push(two_finger_scroll_ended_diagnostic(sequence));
                }
                self.two_phase = TwoFingerPhase::Cancelled;
                self.clear_two_finger_interaction();
                self.two_tap_disqualified = true;
                out.diagnostics
                    .push(two_finger_cancelled_diagnostic("physical click", sequence));
            }
            _ => {
                // No two-finger interaction is active yet, but the physical
                // press still competes with any candidate this frame may
                // anchor (review M9 R2: physical button ownership permanently
                // disqualifies secondary tap for the continuing contact
                // cluster).
                self.two_tap_disqualified = true;
            }
        }
    }

    /// Fail-closed cancellation for timestamp/sequence regression: the
    /// two-finger interaction is cancelled (no further output) but any open
    /// scroll and any held right/latched state **remain**, so the owed
    /// `ScrollEnd` / button releases stay visible to
    /// [`Arbiter::release_all`] (the unconditional escape path). The
    /// continuing contact cluster is also disqualified for secondary tap
    /// (review M9 R3): a later monotonic frame may re-anchor for relative
    /// scroll but must never synthesize a tap.
    fn fail_closed_cancel_two_finger(&mut self) {
        if matches!(
            self.two_phase,
            TwoFingerPhase::Candidate
                | TwoFingerPhase::CommittedScroll
                | TwoFingerPhase::PhysicalSecondaryClickHeld
        ) {
            self.two_phase = TwoFingerPhase::Cancelled;
            self.two_tap_disqualified = true;
            self.clear_two_finger_interaction(); // clears anchors/remainders but NOT scroll_open
        }
    }

    /// M9: a `discontinuity=true` frame ends any active two-finger
    /// interaction (`ScrollEnd` if open; no secondary tap) and may re-anchor
    /// a fresh candidate for future relative scroll — contacts seeded across
    /// the boundary are ineligible for secondary tap because their real down
    /// time and prior movement are unknown. A latched physical secondary
    /// press is not ended by a discontinuity: the physical press must still
    /// be released (physical edges are processed even on discontinuity). The
    /// re-anchor is gated on the scroll capability being enabled (review M9
    /// R1): with scrolling disabled the re-anchor could never serve a
    /// purpose — the pair is tap-disqualified across the recovered boundary.
    /// It is additionally gated on no physical button ownership being held
    /// (review M9 R7): while a physical press is down the continuing cluster
    /// must not re-open a scroll; it may re-anchor relatively only after the
    /// button is cleanly released.
    fn handle_two_finger_discontinuity(
        &mut self,
        frame: &ContactFrame,
        config: &ArbiterConfig,
        out: &mut DraftOutput,
    ) {
        let sequence = frame.sequence;
        if self.two_phase == TwoFingerPhase::PhysicalSecondaryClickHeld {
            return; // the latched press persists until the physical release
        }
        let was_active = matches!(
            self.two_phase,
            TwoFingerPhase::Candidate | TwoFingerPhase::CommittedScroll
        );
        if was_active {
            if self.two_phase == TwoFingerPhase::CommittedScroll && self.scroll_open {
                out.events.push(OutputEvent::ScrollEnd);
                self.scroll_open = false;
                out.diagnostics
                    .push(two_finger_scroll_ended_diagnostic(sequence));
            }
            self.two_phase = TwoFingerPhase::Cancelled;
            // The discontinuity itself disqualifies the continuing contacts
            // for secondary tap until the cluster drains (review M9 R3).
            self.two_tap_disqualified = true;
            out.diagnostics
                .push(two_finger_cancelled_diagnostic("discontinuity", sequence));
            self.clear_two_finger_interaction();
        }
        // Re-anchor: exactly two complete live contacts on this frame may
        // begin a fresh candidate for future relative scroll (their contacts
        // are ineligible for secondary tap) — only when scrolling is enabled.
        // The re-anchor is additionally gated on no physical button ownership
        // being held (review M9 R7): a physical press cancelled the previous
        // interaction, and the continuing cluster must not re-open a scroll
        // while the button remains down; it may re-anchor relatively only
        // after the button is cleanly released.
        let scroll_enabled = config
            .two_finger_config()
            .is_some_and(|c| c.scroll_enabled());
        if scroll_enabled && !self.physical_button_ownership_held() {
            let live_complete: Vec<(i32, (f64, f64))> = frame
                .contacts
                .iter()
                .filter(|c| {
                    matches!(c.state, ContactState::Began | ContactState::Active) && c.is_complete()
                })
                .filter_map(|c| position(c).map(|p| (c.tracking_id, p)))
                .collect();
            if live_complete.len() == 2 {
                let (id1, p1) = live_complete[0];
                let (id2, p2) = live_complete[1];
                self.begin_two_finger_candidate(
                    id1,
                    p1,
                    id2,
                    p2,
                    frame.monotonic_timestamp,
                    sequence,
                    true,
                    out,
                );
            }
        }
    }

    /// M9: the two-finger scroll / secondary-tap / buttonpad
    /// physical-secondary-click policy. Runs after the one-finger match in
    /// [`handle_contacts`], which has already cancelled any one-finger
    /// interaction and released any sticky synthetic-left drag lock on 2+
    /// live contacts, so the two-finger family can own the contacts without
    /// double commit.
    fn handle_two_finger(
        &mut self,
        frame: &ContactFrame,
        config: &ArbiterConfig,
        out: &mut DraftOutput,
    ) -> Result<(), ArbiterError> {
        let live_present = frame
            .contacts
            .iter()
            .any(|c| matches!(c.state, ContactState::Began | ContactState::Active));
        let Some(two_cfg) = config.two_finger_config() else {
            // M9 family disabled (M7/M8 behavior preserved). A fully-ended
            // contact cluster still clears the cluster-level disqualification
            // (set by physical presses), so a genuinely fresh cluster starts
            // tap-eligible.
            if !live_present {
                self.two_tap_disqualified = false;
            }
            return Ok(());
        };
        if !two_cfg.scroll_enabled()
            && !two_cfg.secondary_tap_enabled()
            && !two_cfg.two_finger_physical_click_enabled()
        {
            // A fully-disabled two-finger configuration must not make any
            // capability active merely because an `Option<TwoFingerConfig>`
            // exists (review M9 R1). A fully-ended cluster still clears the
            // cluster-level disqualification.
            if !live_present {
                self.two_tap_disqualified = false;
            }
            return Ok(());
        }

        let interaction_active = matches!(
            self.two_phase,
            TwoFingerPhase::Candidate
                | TwoFingerPhase::CommittedScroll
                | TwoFingerPhase::PhysicalSecondaryClickHeld
        );

        // Cluster drain: when no live contacts remain and no two-finger
        // interaction is still active, the affected contact cluster has fully
        // ended. The cluster-level secondary-tap disqualification (physical
        // button ownership, committed pointer ownership, cancellation,
        // discontinuity, regression — review M9 R2/R3) is lifted here so a
        // genuinely fresh cluster may be tap-eligible again. A frame that is
        // itself ending the last contact keeps the disqualification through
        // its release processing (`interaction_active` is still true), then
        // the next frame observes the drain.
        if !live_present && !interaction_active {
            self.two_tap_disqualified = false;
        }

        let live_complete: Vec<(i32, (f64, f64))> = frame
            .contacts
            .iter()
            .filter(|c| {
                matches!(c.state, ContactState::Began | ContactState::Active) && c.is_complete()
            })
            .filter_map(|c| position(c).map(|p| (c.tracking_id, p)))
            .collect();
        let has_incomplete_live = frame.contacts.iter().any(|c| {
            matches!(c.state, ContactState::Began | ContactState::Active) && !c.is_complete()
        });

        if !interaction_active {
            // No active two-finger interaction: exactly two complete live
            // contacts may form a two-finger candidate (the frame where the
            // second valid contact appears anchors the interaction), but only
            // when a capability that needs the candidate is enabled — scroll
            // or secondary tap (review M9 R1: a disabled capability must not
            // become active). A candidate is also **not** anchored while any
            // physical button ownership is held (review M9 R7): a physical
            // press cancelled the interaction and the continuing cluster must
            // not re-open a scroll while the button remains down — the same
            // still-live pair may establish a fresh relative scroll anchor
            // only after the button is cleanly released (secondary tap stays
            // cluster-disqualified until the cluster drains). The release
            // frame of a latched physical secondary press is exempt: the same
            // fingers that were present during the click must not immediately
            // re-seed a candidate (which could fire a spurious secondary tap
            // on a quick lift right after the click).
            if live_complete.len() == 2
                && !has_incomplete_live
                && !out.latched_right_up
                && !self.physical_button_ownership_held()
                && (two_cfg.scroll_enabled() || two_cfg.secondary_tap_enabled())
            {
                let (id1, p1) = live_complete[0];
                let (id2, p2) = live_complete[1];
                self.begin_two_finger_candidate(
                    id1,
                    p1,
                    id2,
                    p2,
                    frame.monotonic_timestamp,
                    frame.sequence,
                    frame.discontinuity,
                    out,
                );
            }
            return Ok(());
        }

        // A latched physical secondary press owns the interaction: no
        // scroll/tap output while held, regardless of finger-count/contact
        // changes. Only the matching physical release (frame step 4) ends it.
        if self.two_phase == TwoFingerPhase::PhysicalSecondaryClickHeld {
            return Ok(());
        }

        if has_incomplete_live {
            // Missing required coordinates on a live contact: deterministic
            // cancellation (no tap; ScrollEnd if scroll open).
            self.end_two_finger(
                TwoEnd::Cancel("missing required coordinates"),
                frame,
                config,
                out,
            )?;
            return Ok(());
        }

        match live_complete.len() {
            0 | 1 => {
                // Dropped below exactly two fingers: the interaction ends; a
                // qualifying secondary tap fires at most once at this first
                // boundary that ends the exactly-two interaction. A synthetic
                // click additionally requires clean release evidence: at least
                // one anchored pair member must carry a complete `Ended`
                // record at this boundary (its final coordinates count toward
                // displacement); if the missing member simply disappears from
                // the frame, cancel without a click (review M9 R6, mirroring
                // the M8 tap path's `ended_complete` requirement).
                let clean_release = match self.two_finger_ids {
                    Some((lo, hi)) => frame.contacts.iter().any(|c| {
                        (c.tracking_id == lo || c.tracking_id == hi)
                            && c.state == ContactState::Ended
                            && c.is_complete()
                    }),
                    None => false,
                };
                if clean_release {
                    self.end_two_finger(TwoEnd::Release, frame, config, out)?;
                } else {
                    self.end_two_finger(
                        TwoEnd::Cancel("release without Ended record"),
                        frame,
                        config,
                        out,
                    )?;
                }
            }
            2 => {
                let (id1, p1) = live_complete[0];
                let (id2, p2) = live_complete[1];
                if id1 == id2 {
                    // Duplicate identity: degenerate; deterministically cancel.
                    self.end_two_finger(
                        TwoEnd::Cancel("duplicate tracking identity"),
                        frame,
                        config,
                        out,
                    )?;
                } else if self.two_finger_ids == Some(sorted_ids(id1, id2)) {
                    // Same pair (independent of slot/vector order): update the
                    // positions and commit/continue the scroll.
                    self.update_two_finger_pair(
                        id1,
                        p1,
                        id2,
                        p2,
                        frame,
                        two_cfg,
                        config.scroll_fidelity_config(),
                        out,
                    )?;
                } else {
                    // Tracking-id replacement (or an unknown Active contact):
                    // the interaction ends; no tap; ScrollEnd if open. A new
                    // candidate may begin on a later stable frame.
                    self.end_two_finger(
                        TwoEnd::Cancel("tracking id replacement"),
                        frame,
                        config,
                        out,
                    )?;
                }
            }
            _ => {
                // Gained a third (or more) finger: the interaction ends.
                self.end_two_finger(TwoEnd::Cancel("third live contact"), frame, config, out)?;
            }
        }
        Ok(())
    }

    /// The last consumed contact position, when an interaction is active.
    fn last_position(&self) -> Option<(f64, f64)> {
        Some((self.last_x_mm?, self.last_y_mm?))
    }

    /// Begins a new one-finger candidate anchored at `(x, y)` with a zeroed
    /// remainder. Precondition: lifecycle is `Idle`, `Cancelled`, or
    /// `Finished` (validated by [`Arbiter::validate_transition`]).
    ///
    /// M11: the fidelity stage starts fresh for the new interaction (the
    /// replaced interaction's state was already cleared by
    /// [`ArbiterState::clear_interaction`]; this is the defense-in-depth
    /// reset so no fidelity value can leak in).
    fn begin_candidate(&mut self, tracking_id: i32, x: f64, y: f64) {
        debug_assert!(Arbiter::validate_transition(self.lifecycle, Lifecycle::Candidate).is_ok());
        self.lifecycle = Lifecycle::Candidate;
        self.tracking_id = Some(tracking_id);
        self.last_x_mm = Some(x);
        self.last_y_mm = Some(y);
        self.remainder_x_px = 0.0;
        self.remainder_y_px = 0.0;
        self.fidelity = FidelityState::fresh();
    }

    /// Routes one committed one-finger pointer displacement (normalized
    /// millimeters) through the M11 fidelity stage when the configuration
    /// carries one, or the existing linear quantization branch **unchanged**
    /// when it does not (M11_TASK.md §5/§6).
    ///
    /// * fidelity disabled: the existing `quantize(delta * ppm)` branch runs
    ///   exactly as before M11 — no fidelity code is called;
    /// * fidelity enabled: only the committed normalized millimeter delta and
    ///   the frame's monotonic timestamp reach [`fidelity::process`]. `Hold`
    ///   and `Reanchored` emit no pointer event and do **not** alter the
    ///   pixel remainder; `EmitScaledPixels` goes through the existing
    ///   per-axis truncation-toward-zero quantization with the existing
    ///   per-axis remainder. A fidelity runtime error maps fail-closed to
    ///   [`ArbiterError::NonFinite`] and aborts the whole frame draft
    ///   (rollback via the draft).
    ///
    /// The caller advances the last consumed position separately and always —
    /// even on `Hold` — so a held displacement is never re-fed into the
    /// stage (the stage owns the dead-zone accumulation in `P`).
    fn emit_pointer_delta(
        &mut self,
        delta: Mm2,
        routing: PointerRouting,
    ) -> Result<(), ArbiterError> {
        let PointerRouting {
            ppm,
            fidelity,
            timestamp,
            sequence,
            out,
        } = routing;
        match fidelity {
            None => {
                // The existing M10/M7 quantization branch, byte-for-byte the
                // pre-M11 behavior.
                let (emitted_x, remainder_x) = quantize(delta.x * ppm, self.remainder_x_px);
                let (emitted_y, remainder_y) = quantize(delta.y * ppm, self.remainder_y_px);
                self.remainder_x_px = remainder_x;
                self.remainder_y_px = remainder_y;
                push_move(&mut out.events, sequence, emitted_x, emitted_y)?;
            }
            Some(config) => {
                let outcome = crate::fidelity::process(
                    config,
                    &mut self.fidelity,
                    FidelityDeltaMm::new(delta.x, delta.y),
                    timestamp,
                )
                .map_err(|_| ArbiterError::NonFinite { sequence })?;
                match outcome {
                    FidelityOutcome::Hold | FidelityOutcome::Reanchored => {
                        // No pointer event, remainder untouched.
                    }
                    FidelityOutcome::EmitScaledPixels { x, y } => {
                        let (emitted_x, remainder_x) = quantize(x, self.remainder_x_px);
                        let (emitted_y, remainder_y) = quantize(y, self.remainder_y_px);
                        self.remainder_x_px = remainder_x;
                        self.remainder_y_px = remainder_y;
                        push_move(&mut out.events, sequence, emitted_x, emitted_y)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Commits the candidate: routes the accepted accumulated displacement
    /// through the pointer-delta path exactly once and enters `Committed`.
    /// Precondition: lifecycle is `Candidate`.
    fn commit(&mut self, pos: Mm2, acc: Mm2, routing: PointerRouting) -> Result<(), ArbiterError> {
        debug_assert!(Arbiter::validate_transition(self.lifecycle, Lifecycle::Committed).is_ok());
        self.emit_pointer_delta(acc, routing)?;
        self.lifecycle = Lifecycle::Committed;
        self.last_x_mm = Some(pos.x);
        self.last_y_mm = Some(pos.y);
        Ok(())
    }

    /// Commits the candidate and applies **every** pointer-commit side
    /// effect exactly once: the quantized accumulated-displacement movement,
    /// the `Commit` transition, the commit diagnostic, and the M8 tap/drag
    /// ownership update ([`note_pointer_commit`]). Both commitment paths —
    /// an active frame crossing the threshold and a contact crossing it only
    /// in its final `Ended` frame — go through this single helper so the
    /// side effects can never diverge (review M8 R1): a final-frame
    /// commitment must also mark the interaction as a real drag, kill the
    /// first-tap candidate, and keep a locked continuation locked.
    fn commit_pointer(
        &mut self,
        pos: Mm2,
        acc: Mm2,
        ours: i32,
        tap_cfg: Option<&TapConfig>,
        routing: PointerRouting,
    ) -> Result<(), ArbiterError> {
        let PointerRouting {
            ppm,
            fidelity,
            timestamp,
            sequence,
            out,
        } = routing;
        // Safe tap-and-drag: the second contact only becomes a held-left
        // drag once pointer motion really commits. This is intentionally
        // before the first committed delta so output ordering remains
        // ButtonDown(Left) -> Move.
        self.prepare_pointer_commit(tap_cfg, ours, sequence, out);
        // `commit` consumes the routing (and with it the output sink); pass a
        // reborrow so this helper can push the transition/diagnostic after
        // the movement has been emitted.
        self.commit(
            pos,
            acc,
            PointerRouting {
                ppm,
                fidelity,
                timestamp,
                sequence,
                out: &mut *out,
            },
        )?;
        out.transitions
            .push(LifecycleTransition::Commit { tracking_id: ours });
        out.diagnostics.push(commit_diagnostic(ours, sequence));
        self.note_pointer_commit();
        Ok(())
    }

    /// Emits the per-frame (or final Ended-contact) movement for a committed
    /// interaction and advances the last consumed position. Precondition:
    /// lifecycle is `Committed`.
    fn emit_position(&mut self, c: &Contact, routing: PointerRouting) -> Result<(), ArbiterError> {
        let Some((x, y)) = position(c) else {
            return Ok(()); // missing coordinates: nothing to compare
        };
        let Some((lx, ly)) = self.last_position() else {
            return Ok(()); // no baseline: nothing to compare
        };
        self.emit_pointer_delta(
            Mm2 {
                x: x - lx,
                y: y - ly,
            },
            routing,
        )?;
        self.last_x_mm = Some(x);
        self.last_y_mm = Some(y);
        Ok(())
    }

    /// Cancels the active interaction for `ours` and returns its transition.
    /// Precondition: an interaction is active (lifecycle is `Candidate` or
    /// `Committed`), which the caller guarantees.
    fn cancel_interaction(&mut self, ours: i32) -> LifecycleTransition {
        debug_assert!(self.has_interaction());
        debug_assert!(Arbiter::validate_transition(self.lifecycle, Lifecycle::Cancelled).is_ok());
        self.lifecycle = Lifecycle::Cancelled;
        self.clear_interaction();
        LifecycleTransition::Cancel { tracking_id: ours }
    }

    /// Finishes the active interaction (its contact ended). Precondition: an
    /// interaction is active; callers push the `Finish` transition.
    fn finish_interaction(&mut self) {
        debug_assert!(self.has_interaction());
        debug_assert!(Arbiter::validate_transition(self.lifecycle, Lifecycle::Finished).is_ok());
        self.lifecycle = Lifecycle::Finished;
        self.clear_interaction();
    }

    /// Handles the frame's contacts, emitting pointer movement, lifecycle
    /// transitions, tap/drag/lock policy changes, and synthetic button edges
    /// into `out`.
    ///
    /// Runs against the draft: any error aborts the whole frame with no state
    /// committed. The M8 tap/tap-drag/drag-lock policy is inert when the
    /// configuration has no [`TapConfig`] (tapping disabled), preserving M7
    /// behavior exactly.
    fn handle_contacts(
        &mut self,
        frame: &ContactFrame,
        config: &ArbiterConfig,
        out: &mut DraftOutput,
    ) -> Result<(), ArbiterError> {
        let sequence = frame.sequence;
        let ppm = config.logical_pixels_per_mm().as_px_per_mm() as f64;
        let threshold_mm = config.motion_threshold_mm().as_mm() as f64;
        let frame_ts = frame.monotonic_timestamp;
        // M11: the optional fidelity config. `None` (m10-linear-v1 and every
        // pre-M11 config) keeps the existing pointer quantization branch.
        let fidelity = config.fidelity_config();
        let three_finger_drag_fidelity = config.three_finger_drag_fidelity_config().or(fidelity);

        // M15 owns three-finger drag before M14 swipe recognition. Its 1 mm
        // commit threshold is deliberately below the M14 2 mm three-finger
        // swipe threshold. Movement is emitted through the existing M11
        // pointer-fidelity/remainder path; the synthetic-left source uses the
        // existing button multiplexer and cleanup semantics.
        if let Some(drag_cfg) = config.three_finger_drag_config() {
            if frame.discontinuity
                || self.physical_left_raw
                || self.physical_right_raw
                || self.latched_right_owned
            {
                if self.three_finger_drag.phase()
                    != crate::three_finger_drag::ThreeFingerDragPhase::Idle
                {
                    self.three_finger_drag.reset();
                    self.gesture.reset();
                    self.gesture_route.reset();
                    if self.synthetic_left {
                        self.synthetic_left = false;
                        out.synthetic_up = true;
                    }
                }
            } else {
                let drag_contacts: Vec<GestureContact> = frame
                    .contacts
                    .iter()
                    .filter(|contact| {
                        matches!(contact.state, ContactState::Began | ContactState::Active)
                            && contact.is_complete()
                    })
                    .filter_map(|contact| {
                        Some(GestureContact {
                            tracking_id: contact.tracking_id,
                            x_mm: f64::from(contact.x_mm?.as_mm()),
                            y_mm: f64::from(contact.y_mm?.as_mm()),
                            role: self.robustness.role(contact.tracking_id),
                        })
                    })
                    .collect();
                let drag = process_three_finger_drag(
                    drag_cfg,
                    &mut self.three_finger_drag,
                    &drag_contacts,
                    frame_ts,
                );
                if drag.blocks_contact_policy {
                    self.gesture.reset();
                    self.gesture_route.reset();
                    if let Some(ours) = self.tracking_id {
                        if self.has_interaction() {
                            out.transitions.push(self.cancel_interaction(ours));
                        }
                    }
                    self.cancel_tap_policy(out);
                    self.end_two_finger(
                        TwoEnd::Cancel("three-finger drag ownership"),
                        frame,
                        config,
                        out,
                    )?;

                    match drag.action {
                        ThreeFingerDragAction::None => {}
                        ThreeFingerDragAction::BeginDrag { dx_mm, dy_mm } => {
                            // Historical M15-M18 centroid path: commit owns
                            // left immediately and replays the accepted
                            // accumulated displacement exactly once.
                            self.fidelity = FidelityState::fresh();
                            self.remainder_x_px = 0.0;
                            self.remainder_y_px = 0.0;
                            if !self.synthetic_left {
                                self.synthetic_left = true;
                                out.synthetic_down = true;
                            }
                            self.emit_pointer_delta(
                                Mm2 { x: dx_mm, y: dy_mm },
                                PointerRouting {
                                    ppm,
                                    fidelity: three_finger_drag_fidelity,
                                    timestamp: frame_ts,
                                    sequence,
                                    out,
                                },
                            )?;
                        }
                        ThreeFingerDragAction::ArmDrag => {
                            // Commit establishes a fresh drag-motion baseline
                            // only. The pre-commit displacement was used for
                            // classification and must not be replayed after a
                            // synthetic press.
                            self.fidelity = FidelityState::fresh();
                            self.remainder_x_px = 0.0;
                            self.remainder_y_px = 0.0;
                        }
                        ThreeFingerDragAction::Move { dx_mm, dy_mm } => {
                            let events_before = out.events.len();
                            self.emit_pointer_delta(
                                Mm2 { x: dx_mm, y: dy_mm },
                                PointerRouting {
                                    ppm,
                                    fidelity: three_finger_drag_fidelity,
                                    timestamp: frame_ts,
                                    sequence,
                                    out,
                                },
                            )?;
                            // The reference implementation presses only when
                            // a real drag-motion event exists. This avoids a
                            // stationary synthetic click after classification
                            // and guarantees the down and first move share the
                            // same semantic frame.
                            if out.events.len() > events_before && !self.synthetic_left {
                                self.synthetic_left = true;
                                out.synthetic_down = true;
                            }
                        }
                        ThreeFingerDragAction::EndDrag => {
                            if self.synthetic_left {
                                self.synthetic_left = false;
                                out.synthetic_up = true;
                            }
                            self.fidelity = FidelityState::fresh();
                            self.remainder_x_px = 0.0;
                            self.remainder_y_px = 0.0;
                        }
                        ThreeFingerDragAction::Tap => {
                            if let Some(bindings) = config.gesture_bindings_config() {
                                out.events.extend(route_three_finger_tap(bindings));
                            } else {
                                out.events.push(OutputEvent::DesktopAction(
                                    crate::output::DesktopAction::Lookup,
                                ));
                            }
                        }
                    }
                    return Ok(());
                }
            }
        }

        // M14 continuous gestures compete before the lower-priority one- and
        // two-finger owners consume this frame. Candidate recognition is
        // output-free; only a committed gesture (or its terminating frame)
        // blocks the older policies. Thumb metadata comes only from M13's
        // explicit classifier; missing metadata stays `None`.
        if let Some(gesture_cfg) = config.gesture_config() {
            let gesture_contacts: Vec<GestureContact> = frame
                .contacts
                .iter()
                .filter(|contact| {
                    matches!(contact.state, ContactState::Began | ContactState::Active)
                        && contact.is_complete()
                })
                .filter_map(|contact| {
                    Some(GestureContact {
                        tracking_id: contact.tracking_id,
                        x_mm: f64::from(contact.x_mm?.as_mm()),
                        y_mm: f64::from(contact.y_mm?.as_mm()),
                        role: self.robustness.role(contact.tracking_id),
                    })
                })
                .collect();
            let gesture = process_gesture(gesture_cfg, &mut self.gesture, &gesture_contacts);
            if gesture.blocks_contact_policy {
                if let Some(ours) = self.tracking_id {
                    if self.has_interaction() {
                        out.transitions.push(self.cancel_interaction(ours));
                        out.diagnostics
                            .push(cancel_diagnostic("continuous gesture ownership", sequence));
                    }
                }
                self.cancel_tap_policy(out);
                self.end_two_finger(
                    TwoEnd::Cancel("continuous gesture ownership"),
                    frame,
                    config,
                    out,
                )?;
                if let Some(bindings) = config.gesture_bindings_config() {
                    for event in gesture.events {
                        if let Some(event) =
                            route_continuous_gesture(bindings, &mut self.gesture_route, event)
                        {
                            out.events.push(event);
                        }
                    }
                } else {
                    out.events.extend(
                        gesture
                            .events
                            .into_iter()
                            .map(OutputEvent::ContinuousGesture),
                    );
                }
                return Ok(());
            }
        }

        let live: Vec<&Contact> = frame
            .contacts
            .iter()
            .filter(|c| matches!(c.state, ContactState::Began | ContactState::Active))
            .collect();

        match (live.len(), self.lifecycle) {
            // No live contact: the interaction's contact ended (if any).
            (0, Lifecycle::Candidate | Lifecycle::Committed) => {
                let Some(ours) = self.tracking_id else {
                    return Ok(()); // unreachable: invariant guarantees an id
                };
                let ended = frame
                    .contacts
                    .iter()
                    .find(|c| c.tracking_id == ours && c.state == ContactState::Ended);
                if self.lifecycle == Lifecycle::Committed {
                    // The Ended contact carries its final coordinates; emit
                    // the final movement before the interaction finishes.
                    if let Some(ended) = ended {
                        self.emit_position(
                            ended,
                            PointerRouting {
                                ppm,
                                fidelity,
                                timestamp: frame_ts,
                                sequence,
                                out,
                            },
                        )?;
                    }
                } else {
                    // Candidate: the contact may have crossed the threshold
                    // in its final movement (the Ended frame carries the last
                    // position). Commit exactly once — accounting for the
                    // displacement accumulated since the anchor — or, if the
                    // contact ended below threshold, resolve the M8 tap
                    // outcome below (a qualifying tap fires its click pair at
                    // this release frame; otherwise nothing).
                    if let Some(ended) = ended {
                        if let (Some((x, y)), Some((ax, ay))) =
                            (position(ended), self.last_position())
                        {
                            let acc_x = x - ax;
                            let acc_y = y - ay;
                            let magnitude = (acc_x * acc_x + acc_y * acc_y).sqrt();
                            if magnitude >= threshold_mm {
                                // Final-Ended commitment takes the same
                                // pointer-commit side-effect path as an
                                // active-frame commitment (review M8 R1):
                                // movement, Commit transition, diagnostic,
                                // and the M8 tap/drag ownership update.
                                self.commit_pointer(
                                    Mm2 { x, y },
                                    Mm2 { x: acc_x, y: acc_y },
                                    ours,
                                    config.tap_config(),
                                    PointerRouting {
                                        ppm,
                                        fidelity,
                                        timestamp: frame_ts,
                                        sequence,
                                        out,
                                    },
                                )?;
                            }
                        }
                    }
                }
                // M8: resolve the tap/drag/lock outcome at the release frame
                // (tap pulse, second click, drag end, drag-lock entry/exit).
                self.resolve_release_frame(ended, frame_ts, config, ours, sequence, out);
                self.finish_interaction();
                out.transitions
                    .push(LifecycleTransition::Finish { tracking_id: ours });
                out.diagnostics.push(finish_diagnostic(ours, sequence));
            }
            (0, _) => {}

            // One live contact.
            (1, Lifecycle::Idle | Lifecycle::Cancelled | Lifecycle::Finished) => {
                let c = live[0];
                if c.state == ContactState::Began {
                    if let Some((x, y)) = position(c) {
                        // M8 R3: a contact that begins across a stream
                        // discontinuity cannot seed the tap family (its real
                        // touch-down time and pre-recovery movement are
                        // unknown); M7 pointer re-anchoring still proceeds.
                        if frame.discontinuity {
                            self.tap_disqualified = true;
                        }
                        // M8: decide the tap-family interpretation (follow-up
                        // window / locked continuation / first-tap candidate)
                        // before the pointer candidate begins.
                        self.begin_tap_family((x, y), frame_ts, config);
                        self.begin_candidate(c.tracking_id, x, y);
                        out.transitions.push(LifecycleTransition::Begin {
                            tracking_id: c.tracking_id,
                        });
                        out.diagnostics
                            .push(begin_diagnostic(c.tracking_id, sequence));
                    } else {
                        out.diagnostics
                            .push(missing_coordinates_diagnostic(c, sequence));
                    }
                } else if c.state == ContactState::Active {
                    // An Active contact without a known interaction has no
                    // anchor history (e.g. it began during a discontinuity or
                    // a two-finger period) and cannot begin a candidate. M8:
                    // invalid active coordinates while the lock is held with
                    // no contact end the synthetic lock fail-closed.
                    if self.tap_phase == TapDragPhase::LockedWithoutContact && !c.is_complete() {
                        self.cancel_tap_policy(out);
                    }
                }
            }

            (1, Lifecycle::Candidate | Lifecycle::Committed) => {
                let c = live[0];
                let Some(ours) = self.tracking_id else {
                    return Ok(()); // unreachable: invariant guarantees an id
                };
                if c.tracking_id == ours {
                    if !c.is_complete() {
                        // Missing required coordinates on the active contact:
                        // deterministically cancel, no further movement, and
                        // end any synthetic drag/lock fail-closed.
                        let cancelled = self.cancel_interaction(ours);
                        out.transitions.push(cancelled);
                        out.diagnostics
                            .push(cancel_diagnostic("missing required coordinates", sequence));
                        self.cancel_tap_policy(out);
                    } else if self.lifecycle == Lifecycle::Candidate {
                        // Accumulate displacement from the anchor. Commit
                        // exactly once when the magnitude crosses the
                        // threshold, emitting the accepted displacement once.
                        let Some((x, y)) = position(c) else {
                            return Ok(()); // unreachable: is_complete() above
                        };
                        let Some((ax, ay)) = self.last_position() else {
                            return Ok(()); // unreachable: candidate has anchor
                        };
                        let acc_x = x - ax;
                        let acc_y = y - ay;
                        let magnitude = (acc_x * acc_x + acc_y * acc_y).sqrt();
                        // M8: a tap candidate tracks maximum displacement from
                        // its own anchor (not merely the last delta); crossing
                        // the tap threshold permanently disqualifies it.
                        if matches!(
                            self.tap_phase,
                            TapDragPhase::FirstTapCandidate | TapDragPhase::LockedContact
                        ) {
                            self.update_tap_max_displacement(x, y);
                        }
                        if magnitude >= threshold_mm {
                            // Active-frame commitment: movement, Commit
                            // transition, diagnostic, and the M8 tap/drag
                            // ownership update via the shared helper
                            // (review M8 R1).
                            self.commit_pointer(
                                Mm2 { x, y },
                                Mm2 { x: acc_x, y: acc_y },
                                ours,
                                config.tap_config(),
                                PointerRouting {
                                    ppm,
                                    fidelity,
                                    timestamp: frame_ts,
                                    sequence,
                                    out,
                                },
                            )?;
                        }
                    } else {
                        // Committed: emit the per-frame delta.
                        self.emit_position(
                            c,
                            PointerRouting {
                                ppm,
                                fidelity,
                                timestamp: frame_ts,
                                sequence,
                                out,
                            },
                        )?;
                    }
                } else if c.state == ContactState::Began {
                    // Tracking-id replacement (or slot reuse): the previous
                    // contact ended (or vanished) and a new one began in this
                    // frame. Finish the old interaction — emitting its final
                    // committed movement — and start a fresh candidate with a
                    // zeroed remainder.
                    if self.lifecycle == Lifecycle::Committed {
                        if let Some(ended) = frame
                            .contacts
                            .iter()
                            .find(|e| e.tracking_id == ours && e.state == ContactState::Ended)
                        {
                            self.emit_position(
                                ended,
                                PointerRouting {
                                    ppm,
                                    fidelity,
                                    timestamp: frame_ts,
                                    sequence,
                                    out,
                                },
                            )?;
                        }
                    }
                    // M8: resolve the replaced interaction's tap/drag/lock
                    // outcome (a replacement is never a clean tap; a real drag
                    // with lock may continue into locked-without-contact).
                    self.resolve_replacement_old(config, ours, sequence, out);
                    self.finish_interaction();
                    out.transitions
                        .push(LifecycleTransition::Finish { tracking_id: ours });
                    out.diagnostics.push(finish_diagnostic(ours, sequence));
                    if let Some((x, y)) = position(c) {
                        // M8 R3: a replacement contact beginning across a
                        // discontinuity is likewise ineligible for the tap
                        // family (see the `(1, Idle | ...)` Began path).
                        if frame.discontinuity {
                            self.tap_disqualified = true;
                        }
                        self.begin_tap_family((x, y), frame_ts, config);
                        self.begin_candidate(c.tracking_id, x, y);
                        out.transitions.push(LifecycleTransition::Begin {
                            tracking_id: c.tracking_id,
                        });
                        out.diagnostics
                            .push(begin_diagnostic(c.tracking_id, sequence));
                    }
                } else {
                    // An Active contact with a different tracking id than the
                    // interaction we never saw begin: defensive cancel.
                    let cancelled = self.cancel_interaction(ours);
                    out.transitions.push(cancelled);
                    out.diagnostics.push(cancel_diagnostic(
                        "unexpected tracking id replacement",
                        sequence,
                    ));
                    self.cancel_tap_policy(out);
                }
            }

            // Two or more live contacts: cancel the one-finger interaction
            // and emit no further pointer movement from it; end any synthetic
            // drag/lock fail-closed with one logical up (unless physical left
            // holds).
            (_, Lifecycle::Candidate | Lifecycle::Committed) => {
                let Some(ours) = self.tracking_id else {
                    return Ok(()); // unreachable: invariant guarantees an id
                };
                let was_committed = self.lifecycle == Lifecycle::Committed;
                let cancelled = self.cancel_interaction(ours);
                out.transitions.push(cancelled);
                out.diagnostics
                    .push(cancel_diagnostic("second live contact", sequence));
                self.cancel_tap_policy(out);
                // M9 R2: a one-finger interaction that had already committed
                // pointer ownership (emitted PointerMove) before the second
                // finger appeared permanently disqualifies secondary tap for
                // the continuing contact cluster — one continuous cluster
                // must not commit pointer and secondary-tap ownership. A
                // still-candidate one-finger interaction has emitted nothing
                // and does not disqualify.
                if was_committed {
                    self.two_tap_disqualified = true;
                }
            }
            (_, _) => {
                // No active pointer interaction, but the M8 policy may still
                // be live: a second live contact closes the follow-up window
                // ("exactly one new valid finger") and ends a lock held
                // without a contact.
                self.cancel_tap_policy(out);
            }
        }
        // M9: the two-finger scroll / secondary-tap / physical-secondary-click
        // policy runs after the one-finger match. On 2+ live contacts the
        // one-finger interaction and any sticky synthetic-left drag lock have
        // already been cancelled/released above, so the two-finger family owns
        // the contacts without double commit; on 1/0 live contacts a dropped
        // two-finger interaction is ended here (ScrollEnd if open; a
        // qualifying secondary tap fires at most once).
        self.handle_two_finger(frame, config, out)?;
        Ok(())
    }
}

/// The Interaction Arbiter: unified lifecycle + one-finger linear pointer +
/// physical left-button lifecycle (see the [module docs](self)).
#[derive(Clone, Debug, PartialEq)]
pub struct Arbiter {
    config: ArbiterConfig,
    state: ArbiterState,
}

impl Arbiter {
    /// Creates an arbiter with a validated [`ArbiterConfig`].
    ///
    /// The configuration is validated at construction ([`ArbiterConfig::new`]
    /// and [`LogicalPixelsPerMm::try_new`]), so this cannot fail.
    #[must_use]
    pub fn new(config: ArbiterConfig) -> Self {
        Self {
            config,
            state: ArbiterState::fresh(),
        }
    }

    /// The configuration this arbiter was created with.
    #[must_use]
    pub const fn config(&self) -> &ArbiterConfig {
        &self.config
    }

    /// Whether M19 may atomically replace the tunable/user settings without
    /// changing policy in the middle of an owned interaction.
    #[must_use]
    pub fn is_settings_quiescent(&self) -> bool {
        self.state.tracking_id.is_none()
            && self.state.two_finger_ids.is_none()
            && !self.state.scroll_open
            && !self.state.scroll_fidelity.momentum_active()
            && self.state.gesture.is_idle()
            && self.state.three_finger_drag.phase()
                == crate::three_finger_drag::ThreeFingerDragPhase::Idle
            && !self.state.physical_left
            && !self.state.synthetic_left
            && !self.state.physical_right
            && !self.state.synthetic_right
            && !self.state.latched_right_owned
    }

    /// Replaces the complete configuration only at a neutral M19 boundary.
    /// Returns `false` while any relevant ownership is active; callers keep
    /// the latest pending config and retry after later frames.
    pub fn try_replace_config(&mut self, config: ArbiterConfig) -> bool {
        if !self.is_settings_quiescent() {
            return false;
        }
        self.config = config;
        self.state.fidelity = FidelityState::fresh();
        self.state.scroll_fidelity = ScrollFidelityState::default();
        self.state.gesture.reset();
        self.state.gesture_route.reset();
        self.state.three_finger_drag.reset();
        self.state.remainder_x_px = 0.0;
        self.state.remainder_y_px = 0.0;
        self.state.two_remainder_x_px = 0.0;
        self.state.two_remainder_y_px = 0.0;
        true
    }

    /// Supplies an external typing-activity timestamp to the M13 robustness
    /// policy. No OS keyboard state is read inside core; callers that do not
    /// have such a signal simply never call this method.
    pub fn note_typing(&mut self, timestamp: Monotonic) {
        if self.is_dwt_protected_interaction() {
            return;
        }
        if let Some(dwt) = self
            .config
            .robustness_config()
            .map(|robustness| robustness.dwt_config().clone())
        {
            self.state
                .robustness
                .note_typing_with_config(&dwt, timestamp);
        }
    }

    /// Whether current ownership is clearly intentional touch interaction.
    /// Mirroring current libinput DWT behavior, a keyboard event arriving
    /// during committed pointer motion, scrolling, a committed gesture, or
    /// an active drag/button hold must not arm DWT and interrupt that work.
    #[must_use]
    pub fn is_dwt_protected_interaction(&self) -> bool {
        use crate::three_finger_drag::ThreeFingerDragPhase;

        self.state.lifecycle == Lifecycle::Committed
            || self.state.scroll_open
            || self.state.gesture.committed().is_some()
            || matches!(
                self.state.three_finger_drag.phase(),
                ThreeFingerDragPhase::Dragging | ThreeFingerDragPhase::Locked
            )
            || self.state.physical_left
            || self.state.synthetic_left
            || self.state.physical_right
            || self.state.synthetic_right
            || self.state.latched_right_owned
    }

    /// The sticky M13 role for a currently tracked contact, when robustness
    /// is enabled and the contact has been observed.
    #[must_use]
    pub fn contact_role(&self, tracking_id: i32) -> Option<ContactRole> {
        self.state.robustness.role(tracking_id)
    }

    /// The current lifecycle state.
    #[must_use]
    pub const fn lifecycle(&self) -> Lifecycle {
        self.state.lifecycle
    }

    /// The tracking id of the current interaction's contact, when one is
    /// active (lifecycle is `Candidate` or `Committed`).
    #[must_use]
    pub const fn tracking_id(&self) -> Option<i32> {
        self.state.tracking_id
    }

    /// Whether the aggregate left button (physical OR synthetic) is held —
    /// i.e. a `ButtonDown(Left)` was emitted and not yet released.
    #[must_use]
    pub const fn is_left_held(&self) -> bool {
        self.state.physical_left || self.state.synthetic_left
    }

    /// Whether the synthetic left source (tap/tap-and-drag/drag lock) is
    /// currently held.
    #[must_use]
    pub const fn is_synthetic_left_held(&self) -> bool {
        self.state.synthetic_left
    }

    /// Whether the physical left source is currently held.
    #[must_use]
    pub const fn is_physical_left_held(&self) -> bool {
        self.state.physical_left
    }

    /// The M8 tap/drag/drag-lock phase (see [`TapDragPhase`]).
    #[must_use]
    pub const fn tap_drag_phase(&self) -> TapDragPhase {
        self.state.tap_phase
    }

    /// The per-axis unconsumed fractional remainder in pixel space
    /// (`(-1, 1)` on each axis). Exposed for tests that verify the exact
    /// remainder invariant; `0.0` when no interaction is active.
    #[must_use]
    pub const fn remainder_px(&self) -> (f64, f64) {
        (self.state.remainder_x_px, self.state.remainder_y_px)
    }

    /// The M9 two-finger phase (see [`TwoFingerPhase`]).
    #[must_use]
    pub const fn two_finger_phase(&self) -> TwoFingerPhase {
        self.state.two_phase
    }

    /// Whether the aggregate right button (physical OR synthetic OR latched)
    /// is held — i.e. a `ButtonDown(Right)` was emitted and not yet released.
    #[must_use]
    pub const fn is_right_held(&self) -> bool {
        self.state.physical_right || self.state.synthetic_right || self.state.latched_right_owned
    }

    /// Whether the physical right source (driven by
    /// `ContactFrame.physical_buttons.right`) is currently held.
    #[must_use]
    pub const fn is_physical_right_held(&self) -> bool {
        self.state.physical_right
    }

    /// Whether the synthetic right source (two-finger secondary tap pulse) is
    /// currently held. Momentary within a frame for a tap pulse.
    #[must_use]
    pub const fn is_synthetic_right_held(&self) -> bool {
        self.state.synthetic_right
    }

    /// Whether a buttonpad physical-left press is currently latched to the
    /// secondary (right) button for its whole press.
    #[must_use]
    pub const fn is_latched_right_held(&self) -> bool {
        self.state.latched_right_owned
    }

    /// Whether the wire currently carries an open `ScrollBegin` (a committed
    /// two-finger scroll lifecycle that has not yet emitted `ScrollEnd`).
    #[must_use]
    pub const fn is_scroll_open(&self) -> bool {
        self.state.scroll_open
    }

    /// The per-axis unconsumed fractional scroll remainder in pixel space
    /// (`(-1, 1)` on each axis). Exposed for tests that verify the exact
    /// scroll remainder invariant; `0.0` when no two-finger interaction is
    /// active.
    #[must_use]
    pub const fn scroll_remainder_px(&self) -> (f64, f64) {
        (self.state.two_remainder_x_px, self.state.two_remainder_y_px)
    }

    /// Legacy compatibility query. Core no longer owns kinetic scroll after
    /// finger release, so this is always false.
    #[must_use]
    pub const fn is_scroll_momentum_active(&self) -> bool {
        false
    }

    /// Whether a policy timer currently requires periodic monotonic
    /// [`Self::tick`] calls. Today this is the deferred tap release window;
    /// kinetic scroll deliberately lives above the input-policy layer.
    #[must_use]
    pub const fn needs_timer_tick(&self) -> bool {
        matches!(self.state.tap_phase, TapDragPhase::FollowUpWindow)
    }

    /// Timestamp of the last accepted input frame. This is the evdev/trace
    /// time-domain anchor; runtime-generated ticks must be mapped into this
    /// same domain before calling [`Self::tick`].
    #[must_use]
    pub const fn last_input_timestamp(&self) -> Option<Monotonic> {
        self.state.last_timestamp
    }

    /// Sequence number of the last accepted input frame. Runtime clock-domain
    /// bridges use this together with [`Self::last_input_timestamp`] to avoid
    /// re-anchoring on an evdev read that contains events but no new frame.
    #[must_use]
    pub const fn last_input_sequence(&self) -> Option<u64> {
        self.state.last_sequence
    }

    /// Validates a lifecycle transition. Returns
    /// [`TransitionError::Illegal`] for any pair the machine does not
    /// perform in one step, never panics.
    ///
    /// Legal transitions:
    ///
    /// ```text
    /// Idle | Cancelled | Finished -> Candidate   (a new one-finger contact)
    /// Candidate -> Committed | Cancelled | Finished
    /// Committed  -> Cancelled | Finished
    /// ```
    pub fn validate_transition(from: Lifecycle, to: Lifecycle) -> Result<(), TransitionError> {
        let legal = matches!(
            (from, to),
            (
                Lifecycle::Idle | Lifecycle::Cancelled | Lifecycle::Finished,
                Lifecycle::Candidate
            ) | (
                Lifecycle::Candidate,
                Lifecycle::Committed | Lifecycle::Cancelled | Lifecycle::Finished
            ) | (
                Lifecycle::Committed,
                Lifecycle::Cancelled | Lifecycle::Finished
            )
        );
        if legal {
            Ok(())
        } else {
            Err(TransitionError::Illegal { from, to })
        }
    }

    /// Processes one normalized [`ContactFrame`] and returns the ordered
    /// decision.
    ///
    /// **Atomicity:** the entire decision — button edges, pointer movement,
    /// lifecycle transitions, and the new internal state — is computed
    /// against a draft and committed only after every structural check and
    /// every arithmetic step has succeeded. A rejected frame
    /// ([`ArbiterError::InvalidFrame`], [`ArbiterError::NonFinite`]) leaves
    /// the arbiter state untouched. Timestamp/sequence regression reject the
    /// frame *and* deterministically cancel the active interaction (with no
    /// further pointer movement) while retaining the regression baseline.
    ///
    /// This method never touches a sink; it is pure state-machine processing
    /// driven by synthetic or trace-derived frames. Use [`ArbiterSink`] to
    /// feed decisions to an [`OutputSink`].
    pub fn frame(&mut self, frame: &ContactFrame) -> Result<FrameDecision, ArbiterError> {
        let sequence = frame.sequence;

        // 1. Model validation. Consume `ContactFrame::validate()` — the core
        //    model's validation (negative live tracking ids, non-finite
        //    pressure/orientation, negative ellipse axes, duplicate slots) —
        //    rather than a private subset. Any Error/Fatal diagnostic rejects
        //    the frame wholesale: no events and no contact/button/scale/
        //    remainder/lifecycle/baseline change (draft discarded).
        //    Warning-only diagnostics (e.g. an incomplete `Began` contact) do
        //    not reject; the arbiter applies its own warning-only policy below.
        let error_diagnostics: Vec<Diagnostic> = frame
            .validate()
            .into_iter()
            .filter(|d| matches!(d.level, DiagnosticLevel::Error | DiagnosticLevel::Fatal))
            .collect();
        if !error_diagnostics.is_empty() {
            let codes: Vec<DiagnosticCode> = error_diagnostics.iter().map(|d| d.code).collect();
            let reason = error_diagnostics
                .iter()
                .map(|d| d.message.as_str())
                .collect::<Vec<_>>()
                .join("; ");
            return Err(ArbiterError::InvalidFrame {
                sequence,
                codes,
                reason,
            });
        }

        // 2. Regression checks. A regressed frame is rejected, but the active
        //    interaction is deterministically cancelled (explicit M7
        //    requirement) and the regression baseline is retained so the
        //    stream keeps failing closed until release_all. M8: the tap/drag/
        //    lock policy is cancelled fail-closed but any synthetic held state
        //    REMAINS — it must stay visible to release_all, the unconditional
        //    escape path. M9: the two-finger interaction is cancelled
        //    fail-closed but any open scroll and held right/latched state
        //    REMAINS visible to cleanup.
        if let Some(previous) = self.state.last_sequence {
            if sequence <= previous {
                let mut draft = self.state.clone();
                if let Some(ours) = draft.tracking_id {
                    draft.cancel_interaction(ours);
                }
                draft.fail_closed_cancel();
                draft.fail_closed_cancel_two_finger();
                self.state = draft;
                return Err(ArbiterError::SequenceRegression {
                    found: sequence,
                    previous,
                });
            }
        }
        if let Some(previous) = self.state.last_timestamp {
            if frame.monotonic_timestamp < previous {
                let mut draft = self.state.clone();
                if let Some(ours) = draft.tracking_id {
                    draft.cancel_interaction(ours);
                }
                draft.fail_closed_cancel();
                draft.fail_closed_cancel_two_finger();
                self.state = draft;
                return Err(ArbiterError::TimestampRegression {
                    found: frame.monotonic_timestamp,
                    previous,
                });
            }
        }

        // 3. Normal processing against a draft; committed only on success.
        let mut draft = self.state.clone();
        let mut out = DraftOutput {
            events: Vec::new(),
            transitions: Vec::new(),
            diagnostics: Vec::new(),
            synthetic_down: false,
            synthetic_up: false,
            synthetic_right_down: false,
            synthetic_right_up: false,
            latched_right_down: false,
            latched_right_up: false,
        };

        // M13: classification/filtering is part of the same atomic draft as
        // gesture ownership. The original frame has already passed model and
        // regression checks; a filtered clone keeps sequence/timestamp/button
        // state identical while suppressing only contacts whose role demands
        // it and applying bounded jitter to retained contacts.
        let robust_frame = self.config.robustness_config().map(|robustness| {
            robustness_filter_frame(robustness, &mut draft.robustness, frame).frame
        });
        let frame = robust_frame.as_ref().unwrap_or(frame);

        // 4. Physical button edges are consumed atomically with the frame and
        //    are never suppressed by cancellation, added fingers, missing
        //    coordinates, or discontinuity. The pre-frame source states
        //    (`physical_prev`/`synthetic_prev` for left, plus the right
        //    sources) are captured here — the true **pre-frame** states,
        //    before any policy mutation — and the physical sources are applied
        //    to the draft *before* the discontinuity/contact policy runs, so
        //    every synthetic transition below observes the post-frame
        //    physical state. The multiplexer output is therefore derived from
        //    a coherent sequence of source transitions (physical first, then
        //    synthetic/latched), never from stale source snapshots (review M8
        //    R2).
        let physical_prev = draft.physical_left;
        let synthetic_prev = draft.synthetic_left;
        let physical_right_prev = draft.physical_right;
        let synthetic_right_prev = draft.synthetic_right;
        let latched_prev = draft.latched_right_owned;

        // Raw physical edges are tracked on the raw button state (independent
        // of latching, so a latched press's release is still detectable).
        let raw_left = frame.physical_buttons.left;
        let raw_right = frame.physical_buttons.right;
        let physical_down_raw = raw_left && !draft.physical_left_raw;
        let physical_up_raw = !raw_left && draft.physical_left_raw;
        let right_down_raw = raw_right && !draft.physical_right_raw;
        let right_up_raw = !raw_right && draft.physical_right_raw;
        draft.physical_left_raw = raw_left;
        draft.physical_right_raw = raw_right;

        // M9 buttonpad two-finger physical click: a physical-left press while
        // exactly two complete valid fingers are present (and the policy is
        // enabled) is latched to the secondary (right) button for its whole
        // press. The decision is made on the press edge, so a press that began
        // before the second finger appeared remains a primary-left press.
        let live_complete_count = frame
            .contacts
            .iter()
            .filter(|c| {
                matches!(c.state, ContactState::Began | ContactState::Active) && c.is_complete()
            })
            .count();
        let latch_press = physical_down_raw
            && !draft.latched_right_owned
            && self
                .config
                .two_finger_config()
                .is_some_and(|t| t.two_finger_physical_click_enabled())
            && live_complete_count == 2;

        if physical_down_raw {
            if latch_press {
                // The press is diverted to the secondary button: the left
                // source stays false (no ButtonDown(Left)) and the latch owns
                // the right press until the matching physical release.
                draft.latched_right_owned = true;
                out.latched_right_down = true;
                draft.physical_left = false;
                // A two-finger physical click cancels the secondary-tap/scroll
                // candidate and ends a committed scroll; the interaction owns
                // the press (no scroll/tap output while held).
                draft.latch_secondary_press(&mut out, sequence);
                // A real button wins the tap family. In particular, a
                // deferred tap press is released before the secondary press
                // is exposed, preventing a transient Left+Right chord.
                draft.cancel_tap_policy(&mut out);
            } else {
                // Normal physical-left press: M7/M8 behavior preserved.
                draft.physical_left = raw_left;
                // Transfer any pending/active synthetic tap ownership to the
                // physical source without a wire up/down gap: physical_left
                // is already true, so cancel_tap_policy clears synthetic
                // ownership but does not emit a premature ButtonUp.
                draft.cancel_tap_policy(&mut out);
                // M9: a physical press competes with a two-finger
                // candidate/scroll (no tap; ScrollEnd if open).
                draft.cancel_two_finger_for_physical_press(&mut out, sequence);
            }
        } else if physical_up_raw {
            if draft.latched_right_owned {
                // Release the latched secondary press: exactly one
                // ButtonUp(Right) (emitted by the multiplexer unless another
                // right source still holds the wire); never a left up. The
                // per-interaction state was already cleared when the latch
                // began; the tap disqualification set by the click must
                // survive this release so the continuing contacts cannot
                // re-seed a tap-eligible candidate.
                draft.latched_right_owned = false;
                out.latched_right_up = true;
                if draft.two_phase == TwoFingerPhase::PhysicalSecondaryClickHeld {
                    draft.two_phase = TwoFingerPhase::Finished;
                }
                draft.physical_left = false;
            } else {
                draft.physical_left = raw_left;
            }
        } else if !draft.latched_right_owned {
            // Stable physical-left state; while a latched press holds the
            // physical button the left source stays false (the press is
            // diverted).
            draft.physical_left = raw_left;
        }
        if right_down_raw {
            // A physical right press also wins over any pending tap press.
            // The output ordering logic releases left before exposing right.
            draft.cancel_tap_policy(&mut out);
            // A physical right press competes with a two-finger
            // candidate/scroll (no tap; ScrollEnd if open).
            draft.cancel_two_finger_for_physical_press(&mut out, sequence);
        }
        draft.physical_right = raw_right;

        // 5. A discontinuity breaks position continuity: the previous
        //    positions cannot be compared with this frame. Cancel the active
        //    interaction so the motion model re-anchors fresh. M8: a
        //    discontinuity also ends any synthetic drag/lock fail-closed with
        //    one logical up (unless the post-frame physical state still holds
        //    the wire — because physical edges were applied in step 4, this
        //    cancellation sees the *current* physical state, so the up is
        //    recorded exactly when it is owed). M9: a discontinuity ends any
        //    two-finger interaction (`ScrollEnd` if open; no secondary tap)
        //    and may re-anchor a fresh candidate for future relative scroll.
        if frame.discontinuity {
            if let Some(ours) = draft.tracking_id {
                let cancelled = draft.cancel_interaction(ours);
                out.transitions.push(cancelled);
                out.diagnostics
                    .push(cancel_diagnostic("discontinuity", sequence));
            }
            draft.cancel_tap_policy(&mut out);
            draft.handle_two_finger_discontinuity(frame, &self.config, &mut out);
        }

        // 6. M8 timeouts are evaluated at incoming frame boundaries. The
        //    follow-up window expires when a frame arrives strictly after the
        //    deadline (`completed + max_tap_drag_gap`); equality is accepted.
        //    The elapsed time is computed with checked `Duration` arithmetic
        //    (`duration_since`), never saturating addition: the nominal
        //    deadline cannot overflow and overflow can never be silently
        //    converted into a different state transition (review M8 R4). A
        //    frame timestamp earlier than the completed tap is unreachable
        //    here (the regression checks above reject it), but if it ever
        //    occurs the window still fails closed deterministically.
        if draft.tap_phase == TapDragPhase::FollowUpWindow {
            if let (Some(tap_cfg), Some(completed)) =
                (self.config.tap_config(), draft.tap_completed_timestamp)
            {
                let expired = match frame.monotonic_timestamp.duration_since(completed) {
                    Some(elapsed) => elapsed > tap_cfg.max_tap_drag_gap(),
                    None => true, // clock went backwards: fail closed
                };
                if expired {
                    // No continuation arrived: commit the pending click by
                    // releasing the synthetic press now.
                    draft.end_synthetic(&mut out);
                    draft.tap_phase = TapDragPhase::Idle;
                    draft.clear_tap_chain();
                }
            }
        }

        // 7. Contact and tap/drag/lock/two-finger policy processing (may
        //    record synthetic button edges, pointer movement, and scroll
        //    events).
        draft.handle_contacts(frame, &self.config, &mut out)?;
        let DraftOutput {
            events,
            transitions,
            diagnostics,
            synthetic_down,
            synthetic_up,
            synthetic_right_down,
            synthetic_right_up,
            latched_right_down,
            latched_right_up,
        } = out;

        // 8. Button multiplexing: the left sources (physical, synthetic) and
        //    the right sources (physical right, synthetic right, latched
        //    physical-left-as-right) each OR into one aggregate button exposed
        //    to the sink. A `ButtonDown` is emitted only on an aggregate
        //    false->true transition and a `ButtonUp` only on true->false, so a
        //    physical press during a synthetic drag/lock never duplicates a
        //    down, ending a synthetic source never emits an up while another
        //    source still holds the wire, and the same-frame synthetic tap
        //    pulse (left or right) still produces down then up even though the
        //    aggregate begins and ends false. Physical intents are processed
        //    first, so a physical release stays observable even when a
        //    synthetic source takes over or ends in the same frame.
        //
        //    Conditions (derived from the aggregate transition semantics):
        //    * a physical edge moves the aggregate only when no other source
        //      held the wire pre-frame;
        //    * a synthetic edge only emits when no other source holds the
        //      wire post-frame ("no duplicate down" / "no premature up");
        //    * a latched edge (the M9 buttonpad two-finger physical click)
        //      behaves like a physical edge for the right button.
        let mut downs = Vec::new();
        let mut ups = Vec::new();
        if physical_down_raw && !synthetic_prev && !latch_press {
            downs.push(OutputEvent::ButtonDown(MouseButton::Left));
        }
        if physical_up_raw && !synthetic_prev && !latched_prev {
            ups.push(OutputEvent::ButtonUp(MouseButton::Left));
        }
        if synthetic_down && !draft.physical_left {
            downs.push(OutputEvent::ButtonDown(MouseButton::Left));
        }
        if synthetic_up && !draft.physical_left {
            ups.push(OutputEvent::ButtonUp(MouseButton::Left));
        }
        // Right multiplexer (physical right, synthetic right, latched right).
        if right_down_raw && !synthetic_right_prev && !latched_prev {
            downs.push(OutputEvent::ButtonDown(MouseButton::Right));
        }
        if right_up_raw && !synthetic_right_prev && !latched_prev {
            ups.push(OutputEvent::ButtonUp(MouseButton::Right));
        }
        if synthetic_right_down && !raw_right && !draft.latched_right_owned {
            downs.push(OutputEvent::ButtonDown(MouseButton::Right));
        }
        if synthetic_right_up && !raw_right && !draft.latched_right_owned {
            ups.push(OutputEvent::ButtonUp(MouseButton::Right));
        }
        if latched_right_down && !raw_right && !synthetic_right_prev {
            downs.push(OutputEvent::ButtonDown(MouseButton::Right));
        }
        if latched_right_up && !raw_right && !synthetic_right_prev {
            ups.push(OutputEvent::ButtonUp(MouseButton::Right));
        }
        // Deterministic same-frame ordering (review M9 R4). Globally bucketing
        // every button down before policy events and every up after them is
        // correct within one owner, but wrong across an ownership handoff, so
        // ordered intents are assembled instead:
        //   1. pre-handoff releases — a left up that releases old ownership
        //      (sticky drag lock / drag end) in the same frame a right down
        //      takes ownership (latched or physical) must precede it, so the
        //      old owner closes before the new press (never a transient
        //      Left+Right chord). A left up that is part of a same-frame
        //      down+up pulse (tap pulse) is not a handoff release.
        //   2. old-owner scroll closure — a `ScrollEnd` that closes a
        //      committed scroll a physical press is replacing must precede
        //      that press's down: final delta, then `ScrollEnd`, then the new
        //      physical-button down.
        //   3. current-owner button downs — before owned motion ("press
        //      precedes movement").
        //   4. owned motion and lifecycle events (pointer movement,
        //      `ScrollBegin` before first delta).
        //   5. remaining ups — after owned motion ("final movement precedes
        //      release").
        let left_up = |e: &OutputEvent| matches!(e, OutputEvent::ButtonUp(MouseButton::Left));
        let left_down = |e: &OutputEvent| matches!(e, OutputEvent::ButtonDown(MouseButton::Left));
        let right_down = |e: &OutputEvent| matches!(e, OutputEvent::ButtonDown(MouseButton::Right));
        let is_scroll_end = |e: &OutputEvent| matches!(e, OutputEvent::ScrollEnd);
        let left_pulse = downs.iter().any(left_down) && ups.iter().any(left_up);
        // Deferred tap commit can close the previous pending press and start
        // the next pending press in the same release frame. That is an
        // intentional Up->Down re-press, not a Down->Up tap pulse.
        let left_repress = synthetic_prev
            && synthetic_up
            && synthetic_down
            && draft.synthetic_left
            && !draft.physical_left;
        let handoff_left_up = !left_pulse && downs.iter().any(right_down);
        let mut ordered = Vec::with_capacity(events.len() + downs.len() + ups.len());
        if handoff_left_up || left_repress {
            ordered.extend(ups.iter().filter(|e| left_up(e)).cloned());
        }
        ordered.extend(events.iter().filter(|e| is_scroll_end(e)).cloned());
        ordered.extend(downs);
        ordered.extend(events.iter().filter(|e| !is_scroll_end(e)).cloned());
        ordered.extend(
            ups.iter()
                .filter(|e| !((handoff_left_up || left_repress) && left_up(e)))
                .cloned(),
        );

        // The wires must end at the post-frame OR of the sources. Simulate
        // the final semantic ordering (including the deferred-tap re-press)
        // and compare it with the authoritative post-frame aggregate state.
        let (wire_left, wire_right) = simulate_wire(
            physical_prev || synthetic_prev,
            physical_right_prev || synthetic_right_prev || latched_prev,
            ordered.iter(),
        );
        debug_assert_eq!(wire_left, draft.physical_left || draft.synthetic_left);
        debug_assert_eq!(
            wire_right,
            draft.physical_right || draft.synthetic_right || draft.latched_right_owned
        );

        draft.last_sequence = Some(sequence);
        draft.last_timestamp = Some(frame.monotonic_timestamp);
        let tap_drag_phase_after = draft.tap_phase;
        let two_finger_phase_after = draft.two_phase;

        self.state = draft;
        Ok(FrameDecision {
            events: ordered,
            transitions,
            lifecycle_after: self.state.lifecycle,
            tap_drag_phase_after,
            two_finger_phase_after,
            diagnostics,
        })
    }

    /// Advances time-driven interaction policy using an explicitly supplied
    /// monotonic timestamp. Core never reads a clock. Finger-scroll kinetic
    /// continuation is intentionally absent here; the current timer consumer
    /// is deferred tap release. The tick is atomic like [`Self::frame`].
    pub fn tick(&mut self, timestamp: Monotonic) -> Result<FrameDecision, ArbiterError> {
        if let Some(previous) = self.state.last_timestamp {
            if timestamp < previous {
                return Err(ArbiterError::TimestampRegression {
                    found: timestamp,
                    previous,
                });
            }
        }

        let mut draft = self.state.clone();
        let mut events = Vec::new();
        let diagnostics = Vec::new();

        // Ticks are now policy timers only. Two-finger finger scrolling ends
        // on contact release and core never synthesizes kinetic scroll after
        // the fingers leave; the consumer/toolkit owns that continuation.
        // The periodic runtime tick remains useful for libinput-style
        // deferred tap release, which must complete even when no new evdev
        // frame arrives after the tap.
        if draft.tap_phase == TapDragPhase::FollowUpWindow {
            if let (Some(tap_cfg), Some(completed)) =
                (self.config.tap_config(), draft.tap_completed_timestamp)
            {
                let expired = match timestamp.duration_since(completed) {
                    Some(elapsed) => elapsed > tap_cfg.max_tap_drag_gap(),
                    None => true,
                };
                if expired {
                    if draft.synthetic_left {
                        draft.synthetic_left = false;
                        if !draft.physical_left {
                            events.push(OutputEvent::ButtonUp(MouseButton::Left));
                        }
                    }
                    draft.tap_phase = TapDragPhase::Idle;
                    draft.clear_tap_chain();
                }
            }
        }

        let decision = FrameDecision {
            events,
            transitions: Vec::new(),
            lifecycle_after: draft.lifecycle,
            tap_drag_phase_after: draft.tap_phase,
            two_finger_phase_after: draft.two_phase,
            diagnostics,
        };
        self.state = draft;
        Ok(decision)
    }

    /// Idempotent release path, suitable for M10 shutdown.
    ///
    /// Releases the aggregate left button (physical OR synthetic) and the
    /// aggregate right button (physical OR synthetic OR latched) with exactly
    /// one `ButtonUp` each, closes any open scroll with exactly one
    /// `ScrollEnd`, clears the candidate, tap/drag/lock, two-finger scroll,
    /// remainder, and regression baseline, and returns the arbiter to
    /// [`Lifecycle::Idle`], [`TapDragPhase::Idle`], and
    /// [`TwoFingerPhase::Idle`] — even after prior errors. Any synthetic/latched
    /// held state and any open scroll remain visible to this call. The
    /// returned events are the caller's to submit; repeated calls return an
    /// empty list and change nothing.
    #[must_use]
    pub fn release_all(&mut self) -> Vec<OutputEvent> {
        let mut events = Vec::new();
        if self.state.physical_left || self.state.synthetic_left {
            events.push(OutputEvent::ButtonUp(MouseButton::Left));
        }
        if self.state.physical_right || self.state.synthetic_right || self.state.latched_right_owned
        {
            events.push(OutputEvent::ButtonUp(MouseButton::Right));
        }
        if self.state.scroll_open {
            events.push(OutputEvent::ScrollEnd);
        }
        self.state = ArbiterState::fresh();
        events
    }

    /// Aligns the arbiter's button/scroll knowledge with what an output sink
    /// actually accepted. Only [`ArbiterSink`] calls this, after a partial
    /// submission failure or a cleanup attempt: a `ButtonDown`/`ScrollBegin`
    /// the sink rejected must never be tracked as held/open (and must never
    /// produce an unmatched `ButtonUp`/`ScrollEnd`), while an accepted
    /// down/begin whose release failed stays held/open so the cleanup path
    /// retries it. A successful wrapped `release_all()` — which releases all
    /// held state — reconciles to not-held even when the explicit release
    /// submission failed.
    ///
    /// With multiple button sources, the reconciliation adjusts the
    /// *aggregate*: `held == false` clears all sources of that button;
    /// `held == true` keeps the aggregate held through the synthetic source
    /// (the natural owner of an owed release; the physical sources are
    /// input-driven and re-derive from frames).
    fn reconcile(&mut self, left_held: bool, right_held: bool, scroll_open: bool) {
        if left_held {
            self.state.synthetic_left = true;
        } else {
            self.state.physical_left = false;
            self.state.synthetic_left = false;
        }
        if right_held {
            self.state.synthetic_right = true;
        } else {
            self.state.physical_right = false;
            self.state.synthetic_right = false;
            self.state.latched_right_owned = false;
        }
        self.state.scroll_open = scroll_open;
    }
}

/// The contact position in millimetres as f64, or `None` when either
/// coordinate is missing.
fn position(c: &Contact) -> Option<(f64, f64)> {
    Some((c.x_mm?.as_mm() as f64, c.y_mm?.as_mm() as f64))
}

/// Simulates the left/right button wires over an ordered sequence of emitted
/// events, returning the aggregate held state of each button after the last
/// event. Used by `debug_assert`s in [`Arbiter::frame`] to verify the button
/// multiplexers' emitted events leave the wires in the intended post-frame
/// aggregate state.
fn simulate_wire<'a>(
    initial_left: bool,
    initial_right: bool,
    events: impl Iterator<Item = &'a OutputEvent>,
) -> (bool, bool) {
    let (mut left, mut right) = (initial_left, initial_right);
    for event in events {
        match event {
            OutputEvent::ButtonDown(MouseButton::Left) => left = true,
            OutputEvent::ButtonUp(MouseButton::Left) => left = false,
            OutputEvent::ButtonDown(MouseButton::Right) => right = true,
            OutputEvent::ButtonUp(MouseButton::Right) => right = false,
            _ => {}
        }
    }
    (left, right)
}

/// Quantizes one axis of a scaled displacement.
///
/// ```text
/// total        = scaled + previous remainder     (f64)
/// emitted      = trunc(total)                    (toward zero)
/// remainder'   = total - emitted                 (in (-1, 1))
/// ```
///
/// The inputs are always finite (positions are finite `Millimeters`, the
/// scale is finite and positive, and f64 cannot overflow for these ranges),
/// so `total` is finite. The invariant `Σ emitted + remainder == Σ scaled`
/// holds exactly in f64 arithmetic.
fn quantize(scaled: f64, previous_remainder: f64) -> (f64, f64) {
    let total = scaled + previous_remainder;
    debug_assert!(total.is_finite());
    let emitted = total.trunc();
    let remainder = total - emitted;
    (emitted, remainder)
}

/// Converts a quantized per-axis emission to a typed [`LogicalPixels`],
/// failing closed (with the whole frame's atomic rejection) when the value is
/// outside the finite f32 range — only reachable through absurd
/// configuration/input, but checked regardless so a non-finite delta can
/// never be emitted.
fn px_checked(value: f64, sequence: u64) -> Result<LogicalPixels, ArbiterError> {
    if !value.is_finite() || value.abs() > f32::MAX as f64 {
        return Err(ArbiterError::NonFinite { sequence });
    }
    LogicalPixels::try_new(value as f32).map_err(|_| ArbiterError::NonFinite { sequence })
}

/// Pushes a `PointerMove` for a quantized per-axis emission; a fully zero
/// emission produces no event (zero movement produces no `PointerMove`).
fn push_move(
    events: &mut Vec<OutputEvent>,
    sequence: u64,
    emitted_x: f64,
    emitted_y: f64,
) -> Result<(), ArbiterError> {
    if emitted_x == 0.0 && emitted_y == 0.0 {
        return Ok(());
    }
    events.push(OutputEvent::PointerMove {
        dx: px_checked(emitted_x, sequence)?,
        dy: px_checked(emitted_y, sequence)?,
    });
    Ok(())
}

/// Pushes a `ScrollDelta` for a quantized per-axis emission; a fully zero
/// emission produces no event (zero movement produces no `ScrollDelta`).
fn push_scroll_delta(
    events: &mut Vec<OutputEvent>,
    sequence: u64,
    emitted_x: f64,
    emitted_y: f64,
) -> Result<(), ArbiterError> {
    if emitted_x == 0.0 && emitted_y == 0.0 {
        return Ok(());
    }
    events.push(OutputEvent::ScrollDelta {
        dx: px_checked(emitted_x, sequence)?,
        dy: px_checked(emitted_y, sequence)?,
    });
    Ok(())
}

fn begin_diagnostic(tracking_id: i32, sequence: u64) -> Diagnostic {
    Diagnostic::with_frame(
        DiagnosticLevel::Info,
        DiagnosticCode::InteractionBegun,
        format!("one-finger pointer candidate began (tracking id {tracking_id})"),
        sequence,
    )
}

fn commit_diagnostic(tracking_id: i32, sequence: u64) -> Diagnostic {
    Diagnostic::with_frame(
        DiagnosticLevel::Info,
        DiagnosticCode::InteractionCommitted,
        format!("one-finger pointer committed (tracking id {tracking_id})"),
        sequence,
    )
}

fn cancel_diagnostic(reason: &str, sequence: u64) -> Diagnostic {
    Diagnostic::with_frame(
        DiagnosticLevel::Warning,
        DiagnosticCode::InteractionCancelled,
        format!("one-finger interaction cancelled: {reason}"),
        sequence,
    )
}

fn finish_diagnostic(tracking_id: i32, sequence: u64) -> Diagnostic {
    Diagnostic::with_frame(
        DiagnosticLevel::Info,
        DiagnosticCode::InteractionFinished,
        format!("one-finger interaction finished (tracking id {tracking_id})"),
        sequence,
    )
}

fn missing_coordinates_diagnostic(c: &Contact, sequence: u64) -> Diagnostic {
    Diagnostic::with_frame(
        DiagnosticLevel::Warning,
        DiagnosticCode::IncompleteNewContact,
        format!(
            "cannot begin a candidate for contact {} on slot {}: missing required coordinates",
            c.tracking_id, c.slot
        ),
        sequence,
    )
}

fn tap_fired_diagnostic(tracking_id: i32, sequence: u64) -> Diagnostic {
    Diagnostic::with_frame(
        DiagnosticLevel::Info,
        DiagnosticCode::TapFired,
        format!("qualifying tap recognized (tracking id {tracking_id})"),
        sequence,
    )
}

fn tap_and_drag_began_diagnostic(tracking_id: i32, sequence: u64) -> Diagnostic {
    Diagnostic::with_frame(
        DiagnosticLevel::Info,
        DiagnosticCode::TapAndDragBegan,
        format!("tap-and-drag began: synthetic left press for tracking id {tracking_id}"),
        sequence,
    )
}

fn drag_locked_diagnostic(tracking_id: i32, sequence: u64) -> Diagnostic {
    Diagnostic::with_frame(
        DiagnosticLevel::Info,
        DiagnosticCode::DragLocked,
        format!("sticky drag lock engaged (tracking id {tracking_id})"),
        sequence,
    )
}

fn drag_unlocked_diagnostic(tracking_id: i32, sequence: u64) -> Diagnostic {
    Diagnostic::with_frame(
        DiagnosticLevel::Info,
        DiagnosticCode::DragUnlocked,
        format!("sticky drag lock released by a qualifying tap (tracking id {tracking_id})"),
        sequence,
    )
}

fn two_finger_scroll_began_diagnostic(sequence: u64, id_a: i32, id_b: i32) -> Diagnostic {
    Diagnostic::with_frame(
        DiagnosticLevel::Info,
        DiagnosticCode::TwoFingerScrollBegan,
        format!("two-finger candidate began (tracking ids {id_a}, {id_b})"),
        sequence,
    )
}

fn two_finger_scroll_committed_diagnostic(sequence: u64) -> Diagnostic {
    Diagnostic::with_frame(
        DiagnosticLevel::Info,
        DiagnosticCode::TwoFingerScrollCommitted,
        "two-finger scroll committed: ScrollBegin emitted".to_string(),
        sequence,
    )
}

fn two_finger_scroll_ended_diagnostic(sequence: u64) -> Diagnostic {
    Diagnostic::with_frame(
        DiagnosticLevel::Info,
        DiagnosticCode::TwoFingerScrollEnded,
        "two-finger scroll ended: ScrollEnd emitted".to_string(),
        sequence,
    )
}

fn secondary_tap_fired_diagnostic(sequence: u64) -> Diagnostic {
    Diagnostic::with_frame(
        DiagnosticLevel::Info,
        DiagnosticCode::SecondaryTapFired,
        "qualifying two-finger tap emitted its secondary (right) click pair".to_string(),
        sequence,
    )
}

fn two_finger_cancelled_diagnostic(reason: &str, sequence: u64) -> Diagnostic {
    Diagnostic::with_frame(
        DiagnosticLevel::Warning,
        DiagnosticCode::TwoFingerCancelled,
        format!("two-finger interaction cancelled: {reason}"),
        sequence,
    )
}

fn secondary_click_latched_diagnostic(sequence: u64) -> Diagnostic {
    Diagnostic::with_frame(
        DiagnosticLevel::Info,
        DiagnosticCode::SecondaryClickLatched,
        "buttonpad two-finger physical click latched the press to the secondary (right) button"
            .to_string(),
        sequence,
    )
}

/// An arbiter plus an [`OutputSink`]: feeds every decision's events to the
/// sink in order with **explicit delivery acknowledgement**.
///
/// The arbiter is a pure state machine and never touches a sink itself; this
/// adapter is the only place where decisions cross into an [`OutputSink`].
/// Delivery is fail-stop and retryable:
///
/// * Events are submitted one by one and the accepted prefix is tracked. A
///   `ButtonDown` (`Left` or `Right`) is recorded as held only after the sink
///   accepted it, and a `ScrollBegin` only after acceptance opens the scroll
///   lifecycle: a rejected down/begin is **not** treated as delivered and
///   never causes an unmatched up/`ScrollEnd`.
/// * Any partial submission **faults** the adapter: normal
///   [`frame`](Self::frame) calls are rejected until cleanup succeeds,
///   because the output state may have diverged from the decision state.
/// * An accepted down/begin followed by a failed motion/up/end stays known as
///   delivered-held/open. [`release_all`](Self::release_all) submits the
///   matching release(s) exactly once (the *explicit acknowledgement*) and
///   invokes the wrapped sink's own cleanup contract
///   ([`OutputSink::release_all`]). The wrapped cleanup is **authoritative**:
///   a successful wrapped `release_all` releases all held state, so it
///   acknowledges every release/scroll-end even when an explicit submission
///   failed (that failure is still reported, but a later recovery call does
///   not re-submit it). The arbiter resets only at the full acknowledgement
///   boundary — after the explicit releases and the sink cleanup have
///   succeeded. A cleanup that leaves a release unacknowledged (explicit
///   failed *and* wrapped cleanup failed) keeps the owed release retryable
///   and never erases it.
///
/// Production M7/M8/M9 code never instantiates a real desktop output here;
/// tests use [`RecordingSink`](crate::output::RecordingSink) or
/// fault-injecting fakes.
#[derive(Debug)]
pub struct ArbiterSink<S> {
    arbiter: Arbiter,
    sink: S,
    /// Whether the sink has accepted a `ButtonDown(Left)` that has not yet
    /// been released. This is the adapter's delivery knowledge; the arbiter's
    /// aggregate (physical OR synthetic) held state is reconciled to it after
    /// any partial submission or cleanup attempt. A successful wrapped
    /// `release_all()` also clears it — the authoritative acknowledgement —
    /// even when the explicit up submission failed.
    delivered_held_left: bool,
    /// Whether the sink has accepted a `ButtonDown(Right)` that has not yet
    /// been released (M9: secondary tap, latched buttonpad click, or physical
    /// right). Same delivered-knowledge semantics as `delivered_held_left`.
    delivered_held_right: bool,
    /// Whether the sink has accepted a `ScrollBegin` that has not yet been
    /// closed with `ScrollEnd` (M9). A rejected `ScrollBegin` owes no
    /// `ScrollEnd`; an accepted begin followed by a rejected delta/end stays
    /// open and cleanup must close it.
    delivered_scroll_open: bool,
    /// Fail-stop: set after any partial submission; normal frames are blocked
    /// until a successful cleanup reset.
    faulted: bool,
}

impl<S: OutputSink> ArbiterSink<S> {
    /// Creates an adapter around an arbiter and a sink.
    #[must_use]
    pub fn new(config: ArbiterConfig, sink: S) -> Self {
        Self {
            arbiter: Arbiter::new(config),
            sink,
            delivered_held_left: false,
            delivered_held_right: false,
            delivered_scroll_open: false,
            faulted: false,
        }
    }

    /// Whether the adapter is faulted (a previous partial submission has not
    /// yet been cleaned up); normal frames are blocked while faulted.
    #[must_use]
    pub const fn is_faulted(&self) -> bool {
        self.faulted
    }

    /// M19 neutral-boundary configuration replacement. A faulted adapter
    /// never accepts reconfiguration before cleanup.
    pub fn try_replace_config(&mut self, config: ArbiterConfig) -> bool {
        !self.faulted && self.arbiter.try_replace_config(config)
    }

    /// Supplies anonymous typing activity to the arbiter. No output event is
    /// generated directly; it only affects classification of future touches.
    pub fn note_typing(&mut self, timestamp: Monotonic) {
        if !self.faulted {
            self.arbiter.note_typing(timestamp);
        }
    }

    /// Processes one frame and submits the resulting events to the sink,
    /// acknowledging each accepted event.
    ///
    /// Returns [`ArbiterSinkError::Arbiter`] (the adapter stays usable) when
    /// the arbiter rejects the frame; [`ArbiterSinkError::PartialSubmit`] —
    /// entering the faulted state — when the sink rejects an event; or
    /// [`ArbiterSinkError::Faulted`] when a previous partial failure has not
    /// yet been cleaned up.
    pub fn frame(&mut self, frame: &ContactFrame) -> Result<FrameDecision, ArbiterSinkError> {
        if self.faulted {
            return Err(ArbiterSinkError::Faulted);
        }
        // The arbiter commits its decision atomically; an arbiter rejection
        // leaves its state unchanged and never faults the adapter.
        let decision = self.arbiter.frame(frame)?;
        if self.should_split_stable_three_finger_drag_start(&decision.events) {
            // M19's stable-reference three-finger path deliberately gives
            // the ownership edge its own hardware-frame commit before the
            // first relative motion.  linux-3-finger-drag does the same on
            // uinput: BTN_LEFT + SYN_REPORT, then REL_X/REL_Y + SYN_REPORT.
            //
            // Keeping these as two OutputSink::submit_frame calls matters on
            // a fast flick: the first post-classification reference delta can
            // already be several logical pixels.  A compositor must hit-test
            // the press at the pre-motion cursor position rather than being
            // handed one protocol frame containing both the press and that
            // large relative step.  The split is source-specific: ordinary
            // one-finger tap-and-drag still retains its historical paired
            // ButtonDown + PointerMove submission.
            self.submit_event_segment(frame.monotonic_timestamp, &decision.events, 0, 1)?;
            self.submit_event_segment(
                frame.monotonic_timestamp,
                &decision.events,
                1,
                decision.events.len(),
            )?;
        } else {
            self.submit_event_segment(
                frame.monotonic_timestamp,
                &decision.events,
                0,
                decision.events.len(),
            )?;
        }
        Ok(decision)
    }

    /// Whether this decision is the first real motion of M19's
    /// stable-reference three-finger drag.  The exact two-event shape is
    /// intentional: no other event is allowed to be pulled across the
    /// ownership/motion hardware-frame boundary.
    fn should_split_stable_three_finger_drag_start(&self, events: &[OutputEvent]) -> bool {
        let stable_reference = self
            .arbiter
            .config
            .three_finger_drag_config()
            .map(ThreeFingerDragConfig::stable_reference_motion)
            .unwrap_or(false);
        let dragging = self.arbiter.state.three_finger_drag.phase()
            == crate::three_finger_drag::ThreeFingerDragPhase::Dragging;
        stable_reference
            && dragging
            && matches!(
                events,
                [
                    OutputEvent::ButtonDown(MouseButton::Left),
                    OutputEvent::PointerMove { .. }
                ]
            )
    }

    /// Submits one contiguous semantic-event segment and translates a
    /// segment-local accepted prefix/failure index back into the whole
    /// decision's coordinates.  `start` events are known to have committed
    /// before this method is called, which is exactly the state after the
    /// first half of the M19 drag-start split succeeds.
    fn submit_event_segment(
        &mut self,
        timestamp: Monotonic,
        events: &[OutputEvent],
        start: usize,
        end: usize,
    ) -> Result<(), ArbiterSinkError> {
        debug_assert!(start <= end && end <= events.len());
        let segment = &events[start..end];
        if segment.is_empty() {
            return Ok(());
        }
        if let Err(error) = self.sink.submit_frame_at(timestamp, segment) {
            let accepted_in_segment = error.accepted_prefix.min(segment.len());
            for event in &segment[..accepted_in_segment] {
                self.ack_delivered_event(event);
            }
            // A protocol backend may batch several semantic events into one
            // hardware frame. If that frame commit fails, `accepted_prefix`
            // can be earlier than `failed_index`; reconcile from the known
            // committed prefix only.  All events before `start` came from a
            // previously successful segment and are therefore also known to
            // have committed.
            self.faulted = true;
            self.arbiter.reconcile(
                self.delivered_held_left,
                self.delivered_held_right,
                self.delivered_scroll_open,
            );
            let relative_index = error.failed_index.min(segment.len().saturating_sub(1));
            let index = start.saturating_add(relative_index);
            let failed_event = events.get(index).cloned().unwrap_or({
                // A non-empty failure is required by the OutputSink frame
                // contract; this fallback only keeps diagnostics total for a
                // buggy third-party sink.
                OutputEvent::ButtonUp(MouseButton::Left)
            });
            return Err(ArbiterSinkError::PartialSubmit {
                index,
                accepted_prefix: start.saturating_add(accepted_in_segment),
                decision_len: events.len(),
                failed_event,
                primary: error.primary,
            });
        }
        for event in segment {
            self.ack_delivered_event(event);
        }
        Ok(())
    }

    /// Advances input-policy timers (currently deferred tap release) and
    /// submits the resulting events using the same accepted-prefix/fail-stop
    /// contract as [`Self::frame`]. Kinetic scroll is intentionally not
    /// generated here: finger scrolling ends on contact release.
    pub fn tick(&mut self, timestamp: Monotonic) -> Result<FrameDecision, ArbiterSinkError> {
        if self.faulted {
            return Err(ArbiterSinkError::Faulted);
        }
        let decision = self.arbiter.tick(timestamp)?;
        let decision_len = decision.events.len();
        if decision.events.is_empty() {
            return Ok(decision);
        }
        if let Err(error) = self.sink.submit_frame_at(timestamp, &decision.events) {
            let accepted_prefix = error.accepted_prefix.min(decision_len);
            for event in &decision.events[..accepted_prefix] {
                self.ack_delivered_event(event);
            }
            self.faulted = true;
            self.arbiter.reconcile(
                self.delivered_held_left,
                self.delivered_held_right,
                self.delivered_scroll_open,
            );
            let index = error.failed_index.min(decision_len.saturating_sub(1));
            let failed_event = decision
                .events
                .get(index)
                .cloned()
                .unwrap_or(OutputEvent::ButtonUp(MouseButton::Left));
            return Err(ArbiterSinkError::PartialSubmit {
                index,
                accepted_prefix,
                decision_len,
                failed_event,
                primary: error.primary,
            });
        }
        for event in &decision.events {
            self.ack_delivered_event(event);
        }
        Ok(decision)
    }

    fn ack_delivered_event(&mut self, event: &OutputEvent) {
        match event {
            OutputEvent::ButtonDown(MouseButton::Left) => self.delivered_held_left = true,
            OutputEvent::ButtonUp(MouseButton::Left) => self.delivered_held_left = false,
            OutputEvent::ButtonDown(MouseButton::Right) => self.delivered_held_right = true,
            OutputEvent::ButtonUp(MouseButton::Right) => self.delivered_held_right = false,
            OutputEvent::ScrollBegin => self.delivered_scroll_open = true,
            OutputEvent::ScrollEnd => self.delivered_scroll_open = false,
            _ => {}
        }
    }

    /// Releases all held state through the sink (idempotent, retryable).
    ///
    /// Two acknowledgements are involved, with different authority:
    ///
    /// * **Explicit acknowledgement** — for any down the sink accepted, a
    ///   matching `ButtonUp` is submitted exactly once, and for any open
    ///   scroll a `ScrollEnd` exactly once. Only their success clears the
    ///   adapter's delivered-held/open knowledge. Every failed explicit
    ///   release is reported structurally (the first in
    ///   `ReleaseFailed::primary`, the rest in `ReleaseFailed::others` —
    ///   review M9 R5), never collapsed to one error.
    /// * **Authoritative wrapped-cleanup acknowledgement** — the wrapped
    ///   sink's own cleanup ([`OutputSink::release_all`]) is then invoked. Per
    ///   the `OutputSink` contract it releases **all** held button/key state,
    ///   so a successful wrapped cleanup acknowledges every release/scroll-end
    ///   even when an explicit submission failed (that failure is still
    ///   reported, but the delivery knowledge is reconciled to released and a
    ///   later recovery call does not re-submit it). Only when the wrapped
    ///   cleanup *also* fails is an owed release retained and re-submitted on
    ///   the next call.
    ///
    /// Only after both acknowledgements succeed is the arbiter reset to a
    /// fresh interaction state (the acknowledgement boundary) and the fault
    /// cleared. On any failure path the arbiter is **not** reset, so a
    /// failed cleanup can never erase the fact that a release/scroll-end is
    /// still owed; the next call retries exactly what remains unacknowledged.
    pub fn release_all(&mut self) -> Result<(), ArbiterSinkError> {
        // Collect *every* explicit release failure (review M9 R5): M9 can owe
        // `ButtonUp(Right)` and `ScrollEnd` simultaneously, and the structured
        // error must report each failed explicit release — `primary` carries
        // the first (in submission order), `others` the rest — while the
        // retry state (the delivered-held/open flags) and the wrapped cleanup
        // error are preserved exactly.
        let mut primary: Option<OutputError> = None;
        let mut others: Vec<OutputError> = Vec::new();
        let record_failure =
            |primary: &mut Option<OutputError>, others: &mut Vec<OutputError>, err: OutputError| {
                if primary.is_none() {
                    *primary = Some(err);
                } else {
                    others.push(err);
                }
            };
        if self.delivered_held_left {
            match self.sink.submit(OutputEvent::ButtonUp(MouseButton::Left)) {
                Ok(()) => self.delivered_held_left = false,
                Err(err) => record_failure(&mut primary, &mut others, err),
            }
        }
        if self.delivered_held_right {
            match self.sink.submit(OutputEvent::ButtonUp(MouseButton::Right)) {
                Ok(()) => self.delivered_held_right = false,
                Err(err) => record_failure(&mut primary, &mut others, err),
            }
        }
        if self.delivered_scroll_open {
            match self.sink.submit(OutputEvent::ScrollEnd) {
                Ok(()) => self.delivered_scroll_open = false,
                Err(err) => record_failure(&mut primary, &mut others, err),
            }
        }
        let cleanup = self.sink.release_all().err();
        // The wrapped sink's cleanup is the *authoritative* acknowledgement:
        // per the `OutputSink` contract a successful `release_all()` releases
        // ALL held button/key state and is idempotent, so — whether or not
        // the explicit submissions above succeeded — a successful wrapped
        // cleanup reconciles the delivery knowledge to released. A later
        // recovery call must therefore not submit another (duplicate or
        // unmatched) release. Only when the wrapped cleanup *also* failed is
        // an owed release retained and re-submitted on the next call.
        if cleanup.is_none() {
            self.delivered_held_left = false;
            self.delivered_held_right = false;
            self.delivered_scroll_open = false;
        }
        // Keep the arbiter's logical button/scroll state aligned with what
        // the sink actually holds, so the owed releases stay
        // observable/retryable.
        self.arbiter.reconcile(
            self.delivered_held_left,
            self.delivered_held_right,
            self.delivered_scroll_open,
        );
        if primary.is_none() && cleanup.is_none() {
            // Well-defined acknowledgement boundary: every release was
            // accepted. Discard the arbiter's own release events — they
            // mirror only the state we already submitted — and reset for a
            // fresh interaction.
            let _ = self.arbiter.release_all();
            self.faulted = false;
            Ok(())
        } else {
            Err(ArbiterSinkError::ReleaseFailed {
                primary,
                others,
                cleanup,
            })
        }
    }

    /// The underlying arbiter.
    #[must_use]
    pub const fn arbiter(&self) -> &Arbiter {
        &self.arbiter
    }

    /// The underlying sink.
    #[must_use]
    pub const fn sink(&self) -> &S {
        &self.sink
    }

    /// A mutable reference to the underlying sink (M10: the takeover
    /// coordinator prepares a streaming output session through the adapter's
    /// sink after the device is open but before any read or grab).
    #[must_use]
    pub fn sink_mut(&mut self) -> &mut S {
        &mut self.sink
    }

    /// Splits the adapter back into its parts.
    #[must_use]
    pub fn into_parts(self) -> (Arbiter, S) {
        (self.arbiter, self.sink)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contact::PhysicalButtons;
    use crate::output::OutputError;
    use crate::output::RecordingSink;

    fn mm(x: f32) -> Millimeters {
        Millimeters::try_new(x).unwrap()
    }

    fn px(x: f32) -> LogicalPixels {
        LogicalPixels::try_new(x).unwrap()
    }

    /// Default test config: 1 mm motion threshold, 10 px/mm.
    fn cfg() -> ArbiterConfig {
        ArbiterConfig::new(mm(1.0), LogicalPixelsPerMm::try_new(10.0).unwrap()).unwrap()
    }

    fn complete(tracking_id: i32, slot: u32, state: ContactState, x: f32, y: f32) -> Contact {
        let mut c = Contact::new(tracking_id, slot, state);
        c.x_mm = Some(mm(x));
        c.y_mm = Some(mm(y));
        c
    }

    fn began(tracking_id: i32, slot: u32, x: f32, y: f32) -> Contact {
        complete(tracking_id, slot, ContactState::Began, x, y)
    }

    fn active(tracking_id: i32, slot: u32, x: f32, y: f32) -> Contact {
        complete(tracking_id, slot, ContactState::Active, x, y)
    }

    fn ended(tracking_id: i32, slot: u32, x: f32, y: f32) -> Contact {
        complete(tracking_id, slot, ContactState::Ended, x, y)
    }

    fn frm(
        sequence: u64,
        ts: u64,
        contacts: Vec<Contact>,
        buttons: bool,
        discontinuity: bool,
    ) -> ContactFrame {
        ContactFrame {
            monotonic_timestamp: Monotonic::from_nanos(ts),
            sequence,
            discontinuity,
            contacts,
            physical_buttons: PhysicalButtons::new(buttons, false, false),
            diagnostics: vec![],
        }
    }

    fn f(sequence: u64, ts: u64, contacts: Vec<Contact>) -> ContactFrame {
        frm(sequence, ts, contacts, false, false)
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

    /// Feeds frames, panicking on errors; returns all decisions.
    fn run(arbiter: &mut Arbiter, frames: &[ContactFrame]) -> Vec<FrameDecision> {
        frames
            .iter()
            .map(|frame| arbiter.frame(frame).expect("frame must be accepted"))
            .collect()
    }

    /// All PointerMove deltas across decisions, as (dx, dy).
    fn moves(decisions: &[FrameDecision]) -> Vec<(f32, f32)> {
        decisions
            .iter()
            .flat_map(|d| d.events.iter())
            .filter_map(|e| match e {
                OutputEvent::PointerMove { dx, dy } => Some((dx.as_px(), dy.as_px())),
                _ => None,
            })
            .collect()
    }

    /// All Button events across decisions.
    fn buttons(decisions: &[FrameDecision]) -> Vec<OutputEvent> {
        decisions
            .iter()
            .flat_map(|d| d.events.iter())
            .filter(|e| matches!(e, OutputEvent::ButtonDown(_) | OutputEvent::ButtonUp(_)))
            .cloned()
            .collect()
    }

    // ------------------------------------------------------------------
    // Lifecycle transitions
    // ------------------------------------------------------------------

    #[test]
    fn transition_table_legal_and_illegal_pairs() {
        use Lifecycle::*;
        let legal: &[(Lifecycle, Lifecycle)] = &[
            (Idle, Candidate),
            (Cancelled, Candidate),
            (Finished, Candidate),
            (Candidate, Committed),
            (Candidate, Cancelled),
            (Candidate, Finished),
            (Committed, Cancelled),
            (Committed, Finished),
        ];
        let all = [Idle, Candidate, Committed, Cancelled, Finished];
        for from in all {
            for to in all {
                let expected = legal.contains(&(from, to));
                assert_eq!(
                    Arbiter::validate_transition(from, to).is_ok(),
                    expected,
                    "transition {from:?} -> {to:?}"
                );
                if !expected {
                    let err = Arbiter::validate_transition(from, to).unwrap_err();
                    assert_eq!(err.from(), from);
                    assert_eq!(err.to(), to);
                }
            }
        }
    }

    #[test]
    fn lifecycle_walks_begin_commit_finish() {
        let mut a = Arbiter::new(cfg());
        assert_eq!(a.lifecycle(), Lifecycle::Idle);

        let d0 = run(&mut a, &[f(0, 0, vec![began(1, 0, 0.0, 0.0)])]);
        assert_eq!(d0[0].lifecycle_after, Lifecycle::Candidate);
        assert_eq!(
            d0[0].transitions,
            vec![LifecycleTransition::Begin { tracking_id: 1 }]
        );

        let d1 = run(&mut a, &[f(1, 1, vec![active(1, 0, 2.0, 0.0)])]);
        assert_eq!(d1[0].lifecycle_after, Lifecycle::Committed);
        assert_eq!(
            d1[0].transitions,
            vec![LifecycleTransition::Commit { tracking_id: 1 }]
        );

        let d2 = run(&mut a, &[f(2, 2, vec![ended(1, 0, 2.0, 0.0)])]);
        assert_eq!(d2[0].lifecycle_after, Lifecycle::Finished);
        assert_eq!(
            d2[0].transitions,
            vec![LifecycleTransition::Finish { tracking_id: 1 }]
        );
    }

    #[test]
    fn lifecycle_walks_begin_cancel_then_fresh_begin() {
        let mut a = Arbiter::new(cfg());
        run(&mut a, &[f(0, 0, vec![began(1, 0, 0.0, 0.0)])]);
        // Second contact cancels the candidate.
        let d = run(
            &mut a,
            &[f(1, 1, vec![active(1, 0, 0.2, 0.0), began(2, 1, 5.0, 5.0)])],
        );
        assert_eq!(d[0].lifecycle_after, Lifecycle::Cancelled);
        assert_eq!(
            d[0].transitions,
            vec![LifecycleTransition::Cancel { tracking_id: 1 }]
        );
        // A fresh touch begins a new candidate.
        let d = run(&mut a, &[f(2, 2, vec![began(3, 0, 1.0, 1.0)])]);
        assert_eq!(d[0].lifecycle_after, Lifecycle::Candidate);
        assert_eq!(
            d[0].transitions,
            vec![LifecycleTransition::Begin { tracking_id: 3 }]
        );
    }

    // ------------------------------------------------------------------
    // Candidate period and threshold
    // ------------------------------------------------------------------

    #[test]
    fn begin_active_end_below_threshold_produces_no_output() {
        let mut a = Arbiter::new(cfg());
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.5, 0.2)]),
                f(2, 2, vec![ended(1, 0, 0.5, 0.2)]),
            ],
        );
        assert!(d.iter().all(|d| d.events.is_empty()));
        assert_eq!(d[2].lifecycle_after, Lifecycle::Finished);
        assert_eq!(
            d[2].transitions,
            vec![LifecycleTransition::Finish { tracking_id: 1 }]
        );
        assert_eq!(a.lifecycle(), Lifecycle::Finished);
    }

    #[test]
    fn exact_threshold_boundary_commits() {
        let mut a = Arbiter::new(cfg()); // threshold exactly 1.0 mm
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 1.0, 0.0)]),
            ],
        );
        assert_eq!(d[1].lifecycle_after, Lifecycle::Committed);
        assert_eq!(moves(&d), vec![(10.0, 0.0)]);
    }

    #[test]
    fn just_below_threshold_does_not_commit() {
        let mut a = Arbiter::new(cfg());
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.9999, 0.0)]),
            ],
        );
        assert_eq!(d[1].lifecycle_after, Lifecycle::Candidate);
        assert!(d[1].events.is_empty());
    }

    #[test]
    fn just_over_threshold_commits() {
        let mut a = Arbiter::new(cfg());
        // 1.25 mm > 1.0 mm threshold; 1.25 is exact in binary.
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 1.25, 0.0)]),
            ],
        );
        assert_eq!(d[1].lifecycle_after, Lifecycle::Committed);
        assert_eq!(moves(&d), vec![(12.0, 0.0)]);
        assert_eq!(a.remainder_px(), (0.5, 0.0));
    }

    #[test]
    fn candidate_crossing_threshold_in_final_ended_frame_commits_once() {
        // The contact crosses the 1 mm threshold only in its Ended frame.
        let mut a = Arbiter::new(cfg());
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.8, 0.0)]), // below threshold
                f(2, 2, vec![ended(1, 0, 1.5, 0.0)]),  // crosses in the final frame
            ],
        );
        assert_eq!(moves(&d), vec![(15.0, 0.0)]);
        assert_eq!(
            d[2].transitions,
            vec![
                LifecycleTransition::Commit { tracking_id: 1 },
                LifecycleTransition::Finish { tracking_id: 1 },
            ]
        );
        assert_eq!(d[2].lifecycle_after, Lifecycle::Finished);
    }

    // ------------------------------------------------------------------
    // Motion: axes, signs, zero, accumulation
    // ------------------------------------------------------------------

    #[test]
    fn horizontal_vertical_diagonal_negative_motion() {
        // Horizontal commit then vertical/negative incremental motion.
        let mut a = Arbiter::new(cfg());
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 2.0, 0.0)]), // commit: (20, 0)
                f(2, 2, vec![active(1, 0, 2.0, -1.5)]), // delta: (0, -15)
                f(3, 3, vec![active(1, 0, 0.5, -1.5)]), // delta: (-15, 0)
                f(4, 4, vec![active(1, 0, 1.5, 2.5)]), // delta: (10, 40)
            ],
        );
        assert_eq!(
            moves(&d),
            vec![(20.0, 0.0), (0.0, -15.0), (-15.0, 0.0), (10.0, 40.0)]
        );
    }

    #[test]
    fn diagonal_commit_emits_both_axes_once() {
        let mut a = Arbiter::new(cfg());
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 1.5, 2.0)]),
            ],
        );
        assert_eq!(moves(&d), vec![(15.0, 20.0)]);
    }

    #[test]
    fn zero_movement_produces_no_pointer_move() {
        let mut a = Arbiter::new(cfg());
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 2.0, 0.0)]), // commit
                f(2, 2, vec![active(1, 0, 2.0, 0.0)]), // zero delta: no event
                f(3, 3, vec![active(1, 0, 2.0, 0.0)]), // zero delta: no event
            ],
        );
        assert_eq!(moves(&d), vec![(20.0, 0.0)]);
        assert!(d[2].events.is_empty() && d[3].events.is_empty());
    }

    #[test]
    fn first_committed_delta_accounts_exactly_once_then_incremental() {
        let mut a = Arbiter::new(cfg());
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.5, 0.0)]), // below threshold: nothing
                f(2, 2, vec![active(1, 0, 1.2, 0.5)]), // commit: (12, 5) exactly once
                f(3, 3, vec![active(1, 0, 1.5, 0.5)]), // incremental: (3, 0)
                f(4, 4, vec![active(1, 0, 1.5, 0.5)]), // zero: nothing
                f(5, 5, vec![active(1, 0, 1.5, 1.0)]), // incremental: (0, 5)
            ],
        );
        assert_eq!(moves(&d), vec![(12.0, 5.0), (3.0, 0.0), (0.0, 5.0)]);
        // The accumulated displacement appears exactly once.
        assert_eq!(moves(&d).iter().filter(|m| **m == (12.0, 5.0)).count(), 1);
    }

    // ------------------------------------------------------------------
    // Sub-pixel remainder invariant
    // ------------------------------------------------------------------

    #[test]
    fn many_small_deltas_equal_one_aggregate_delta() {
        // Total displacement 2.0 mm, ppm 10 -> 20 px.
        let cfg_small =
            ArbiterConfig::new(mm(0.5), LogicalPixelsPerMm::try_new(10.0).unwrap()).unwrap();
        let mut small = Arbiter::new(cfg_small.clone());
        let mut frames = vec![f(0, 0, vec![began(1, 0, 0.0, 0.0)])];
        for i in 0..8 {
            frames.push(f(
                i as u64 + 1,
                i as u64 + 1,
                vec![active(1, 0, 0.25 * (i as f32 + 1.0), 0.0)],
            ));
        }
        let d_small = run(&mut small, &frames);
        let total_small: f32 = moves(&d_small).iter().map(|(x, _)| x).sum();

        let mut aggregate = Arbiter::new(cfg_small.clone());
        let d_agg = run(
            &mut aggregate,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 2.0, 0.0)]),
            ],
        );
        let total_agg: f32 = moves(&d_agg).iter().map(|(x, _)| x).sum();
        assert_eq!(total_small, 20.0);
        assert_eq!(total_agg, 20.0);
        assert_eq!(total_small, total_agg);
    }

    #[test]
    fn fractional_scale_small_deltas_equal_aggregate() {
        // ppm 3.5, total 2.0 mm -> 7.0 px; 5 x 0.4 mm.
        let cfg_s = ArbiterConfig::new(mm(1.0), LogicalPixelsPerMm::try_new(3.5).unwrap()).unwrap();
        let mut small = Arbiter::new(cfg_s.clone());
        let mut frames = vec![f(0, 0, vec![began(1, 0, 0.0, 0.0)])];
        for i in 0..5 {
            frames.push(f(
                i as u64 + 1,
                i as u64 + 1,
                vec![active(1, 0, 0.4 * (i as f32 + 1.0), 0.0)],
            ));
        }
        let d_small = run(&mut small, &frames);
        let total_small: f32 = moves(&d_small).iter().map(|(x, _)| x).sum();

        let mut aggregate = Arbiter::new(cfg_s.clone());
        let d_agg = run(
            &mut aggregate,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 2.0, 0.0)]),
            ],
        );
        let total_agg: f32 = moves(&d_agg).iter().map(|(x, _)| x).sum();
        assert_eq!(total_small, 7.0);
        assert_eq!(total_agg, 7.0);
        assert_eq!(total_small, total_agg);
    }

    #[test]
    fn remainder_accumulates_and_is_reported() {
        // ppm 10: a 0.25 mm delta is 2.5 px -> emits 2, remainder 0.5.
        let mut a = Arbiter::new(cfg());
        run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 2.0, 0.0)]),
            ],
        );
        assert_eq!(a.remainder_px(), (0.0, 0.0));
        // 0.25 mm * 10 = 2.5 px: emits 2, remainder 0.5.
        let d = run(&mut a, &[f(2, 2, vec![active(1, 0, 2.25, 0.0)])]);
        assert_eq!(moves(&d), vec![(2.0, 0.0)]);
        assert_eq!(a.remainder_px(), (0.5, 0.0));
        // Another 0.25 mm: total 3.0 px -> emits 3, remainder 0.
        let d = run(&mut a, &[f(3, 3, vec![active(1, 0, 2.5, 0.0)])]);
        assert_eq!(moves(&d), vec![(3.0, 0.0)]);
        assert_eq!(a.remainder_px(), (0.0, 0.0));
    }

    #[test]
    fn negative_remainder_is_carried_symmetrically() {
        let mut a = Arbiter::new(cfg());
        run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, -2.0, 0.0)]),
            ],
        );
        // -0.25 mm * 10 = -2.5 px -> emits -2, remainder -0.5.
        let d = run(&mut a, &[f(2, 2, vec![active(1, 0, -2.25, 0.0)])]);
        assert_eq!(moves(&d), vec![(-2.0, 0.0)]);
        assert_eq!(a.remainder_px(), (-0.5, 0.0));
        // Another -0.25 mm: total -3.0 px -> emits -3, remainder 0.
        let d = run(&mut a, &[f(3, 3, vec![active(1, 0, -2.5, 0.0)])]);
        assert_eq!(moves(&d), vec![(-3.0, 0.0)]);
        assert_eq!(a.remainder_px(), (0.0, 0.0));
    }

    #[test]
    fn quantize_math_is_exact() {
        assert_eq!(quantize(10.0, 0.0), (10.0, 0.0));
        assert_eq!(quantize(10.5, 0.0), (10.0, 0.5));
        assert_eq!(quantize(0.5, 0.0), (0.0, 0.5));
        assert_eq!(quantize(-0.5, 0.0), (0.0, -0.5));
        assert_eq!(quantize(0.4, 0.6), (1.0, 0.0)); // carries across
                                                    // Σ emitted + remainder == Σ scaled over a sequence.
        let scaled = [0.4, 0.4, 0.4, -0.5, 0.8];
        let mut rem = 0.0;
        let mut emitted_total = 0.0;
        let mut scaled_total = 0.0;
        for s in scaled {
            scaled_total += s;
            let (e, r) = quantize(s, rem);
            emitted_total += e;
            rem = r;
        }
        assert!((emitted_total + rem - scaled_total).abs() < 1e-12);
        assert!((-1.0..1.0).contains(&rem));
    }

    // ------------------------------------------------------------------
    // Tracking-id replacement and slot reuse
    // ------------------------------------------------------------------

    #[test]
    fn tracking_id_replacement_resets_anchor_and_remainder() {
        // Old interaction carries a 0.9 px remainder; the new interaction must
        // not inherit it (0.2 px fresh -> emits 0, not 1).
        let mut a = Arbiter::new(cfg());
        run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 2.0, 0.0)]), // commit (20, 0)
                f(2, 2, vec![active(1, 0, 2.25, 0.0)]), // emits 2, remainder 0.5
                // The decoder replaces the tracking id: only the new Began
                // contact appears (old lifecycles never share a slot).
                f(3, 3, vec![began(2, 0, 10.0, 10.0)]),
            ],
        );
        assert_eq!(a.lifecycle(), Lifecycle::Candidate);
        assert_eq!(a.remainder_px(), (0.0, 0.0)); // residue reset
                                                  // The new interaction commits with a fresh remainder: 2.0 mm * 10 =
                                                  // 20.0 px exactly, leaving remainder (0, 0). If the old 0.5 px
                                                  // remainder had leaked into the commit, the remainder would be 0.5.
        let d = run(&mut a, &[f(4, 4, vec![active(2, 0, 12.0, 10.0)])]);
        assert_eq!(moves(&d), vec![(20.0, 0.0)]);
        assert_eq!(a.remainder_px(), (0.0, 0.0));
        // 0.25 mm * 10 = 2.5 px: with a fresh remainder this emits 2.
        // If the old 0.5 px remainder had leaked, 0.5 + 2.5 = 3.0 would emit 3.
        let d = run(&mut a, &[f(5, 5, vec![active(2, 0, 12.25, 10.0)])]);
        assert_eq!(moves(&d), vec![(2.0, 0.0)]);
        assert_eq!(a.remainder_px(), (0.5, 0.0));
    }

    #[test]
    fn slot_reuse_without_anchor_leakage() {
        let mut a = Arbiter::new(cfg());
        run(
            &mut a,
            &[
                f(0, 0, vec![began(7, 3, 0.0, 0.0)]),
                f(1, 1, vec![active(7, 3, 2.0, 0.0)]),
                f(2, 2, vec![ended(7, 3, 2.0, 0.0)]),
                f(3, 3, vec![began(8, 3, 5.0, 5.0)]), // same slot, new id
                f(4, 4, vec![active(8, 3, 5.5, 5.0)]),
            ],
        );
        // The new interaction's anchor is (5,5): only (5,0) of motion, not the
        // old (2,0) origin.
        let d = run(&mut a, &[f(5, 5, vec![active(8, 3, 5.0, 5.0)])]);
        assert!(d[0].events.is_empty());
        assert_eq!(a.lifecycle(), Lifecycle::Candidate); // 0.5 < 1.0 threshold
    }

    #[test]
    fn active_contact_after_cancel_does_not_resume() {
        let mut a = Arbiter::new(cfg());
        run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 2.0, 0.0)]), // committed
                f(2, 2, vec![active(1, 0, 2.0, 0.0), began(2, 1, 5.0, 5.0)]), // cancel
                f(3, 3, vec![active(1, 0, 4.0, 0.0)]), // one finger again
            ],
        );
        assert_eq!(a.lifecycle(), Lifecycle::Cancelled);
        // Only a fresh Began contact starts a new interaction.
        let d = run(&mut a, &[f(4, 4, vec![began(3, 0, 0.0, 0.0)])]);
        assert_eq!(d[0].lifecycle_after, Lifecycle::Candidate);
    }

    // ------------------------------------------------------------------
    // Second contact
    // ------------------------------------------------------------------

    #[test]
    fn second_contact_before_commitment_cancels_without_motion() {
        let mut a = Arbiter::new(cfg());
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.5, 0.0), began(2, 1, 5.0, 5.0)]),
            ],
        );
        assert_eq!(d[1].lifecycle_after, Lifecycle::Cancelled);
        assert_eq!(
            d[1].transitions,
            vec![LifecycleTransition::Cancel { tracking_id: 1 }]
        );
        assert!(d[1].events.is_empty());
        assert!(moves(&d).is_empty());
    }

    #[test]
    fn second_contact_after_commitment_cancels_with_no_further_motion() {
        let mut a = Arbiter::new(cfg());
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 2.0, 0.0)]), // commit (20, 0)
                f(2, 2, vec![active(1, 0, 3.0, 0.0), began(2, 1, 5.0, 5.0)]), // cancel
                f(3, 3, vec![active(1, 0, 4.0, 0.0)]), // no further movement
            ],
        );
        assert_eq!(moves(&d), vec![(20.0, 0.0)]);
        assert_eq!(d[2].lifecycle_after, Lifecycle::Cancelled);
        assert_eq!(
            d[2].transitions,
            vec![LifecycleTransition::Cancel { tracking_id: 1 }]
        );
        assert!(d[2].events.is_empty() && d[3].events.is_empty());
    }

    #[test]
    fn second_finger_lifting_alone_does_not_cancel() {
        // A second finger begins and ends without overlapping a live frame of
        // ours: the Ended entry alone must not cancel the interaction.
        let mut a = Arbiter::new(cfg());
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 2.0, 0.0), ended(2, 1, 5.0, 5.0)]),
            ],
        );
        assert_eq!(d[1].lifecycle_after, Lifecycle::Committed);
        assert_eq!(moves(&d), vec![(20.0, 0.0)]);
    }

    // ------------------------------------------------------------------
    // Invalid frames, discontinuity, regression
    // ------------------------------------------------------------------

    #[test]
    fn duplicate_slot_frame_is_rejected_without_state_change() {
        let mut a = Arbiter::new(cfg());
        run(&mut a, &[f(0, 0, vec![began(1, 0, 0.0, 0.0)])]);
        let mut bad = f(1, 1, vec![active(1, 0, 2.0, 0.0), active(1, 0, 3.0, 0.0)]);
        // Force a duplicate slot: both contacts on slot 0.
        bad.contacts[1].slot = 0;
        let err = a.frame(&bad).unwrap_err();
        assert!(matches!(
            err,
            ArbiterError::InvalidFrame {
                sequence: 1,
                ref codes,
                ..
            } if codes.contains(&DiagnosticCode::DuplicateSlot)
        ));
        // State unchanged: still a candidate at the old anchor.
        assert_eq!(a.lifecycle(), Lifecycle::Candidate);
        assert_eq!(a.tracking_id(), Some(1));
        // A subsequent valid frame continues normally.
        let d = run(&mut a, &[f(2, 2, vec![active(1, 0, 2.0, 0.0)])]);
        assert_eq!(d[0].lifecycle_after, Lifecycle::Committed);
    }

    // ------------------------------------------------------------------
    // R2: model validation (ContactFrame::validate) rejection
    // ------------------------------------------------------------------

    /// A live contact with a negative tracking id is an Error diagnostic from
    /// the core model; the arbiter rejects the frame atomically.
    #[test]
    fn negative_live_tracking_id_frame_is_rejected_atomically() {
        let mut a = Arbiter::new(cfg());
        run(&mut a, &[f(0, 0, vec![began(1, 0, 0.0, 0.0)])]);
        let bad = f(1, 1, vec![Contact::new(-3, 0, ContactState::Active)]);
        let err = a.frame(&bad).unwrap_err();
        assert!(matches!(
            err,
            ArbiterError::InvalidFrame {
                sequence: 1,
                ref codes,
                ..
            } if codes.contains(&DiagnosticCode::InvalidEventOrder)
        ));
        // State unchanged: still a candidate at the old anchor.
        assert_eq!(a.lifecycle(), Lifecycle::Candidate);
        assert_eq!(a.tracking_id(), Some(1));
        assert_eq!(a.remainder_px(), (0.0, 0.0));
        // A subsequent valid frame continues normally.
        let d = run(&mut a, &[f(2, 2, vec![active(1, 0, 2.0, 0.0)])]);
        assert_eq!(d[0].lifecycle_after, Lifecycle::Committed);
    }

    /// Non-finite/out-of-range pressure, non-finite orientation, and negative
    /// ellipse axes are Error diagnostics; each rejects the frame wholesale.
    #[test]
    fn invalid_pressure_orientation_ellipse_frames_are_rejected() {
        // Pressure outside [0, 1].
        let mut c = began(1, 0, 0.0, 0.0);
        c.pressure = Some(1.5);
        let err = Arbiter::new(cfg()).frame(&f(0, 0, vec![c])).unwrap_err();
        assert!(matches!(
            err,
            ArbiterError::InvalidFrame {
                sequence: 0,
                ref codes,
                ..
            } if codes.contains(&DiagnosticCode::OutOfRangeValue)
        ));

        // Non-finite pressure.
        let mut c = began(1, 0, 0.0, 0.0);
        c.pressure = Some(f32::NAN);
        let err = Arbiter::new(cfg()).frame(&f(0, 0, vec![c])).unwrap_err();
        assert!(matches!(
            err,
            ArbiterError::InvalidFrame {
                ref codes,
                ..
            } if codes.contains(&DiagnosticCode::NonFiniteValue)
        ));

        // Non-finite orientation.
        let mut c = began(1, 0, 0.0, 0.0);
        c.orientation = Some(f32::INFINITY);
        let err = Arbiter::new(cfg()).frame(&f(0, 0, vec![c])).unwrap_err();
        assert!(matches!(
            err,
            ArbiterError::InvalidFrame {
                ref codes,
                ..
            } if codes.contains(&DiagnosticCode::NonFiniteValue)
        ));

        // Negative ellipse major axis.
        let mut c = began(1, 0, 0.0, 0.0);
        c.major_mm = Some(mm(-0.5));
        let err = Arbiter::new(cfg()).frame(&f(0, 0, vec![c])).unwrap_err();
        assert!(matches!(
            err,
            ArbiterError::InvalidFrame {
                ref codes,
                ..
            } if codes.contains(&DiagnosticCode::OutOfRangeValue)
        ));
    }

    /// An incomplete `Began` contact is a Warning-only diagnostic: the frame
    /// is accepted, no candidate is created, and the arbiter emits its own
    /// warning diagnostic (the M7 warning-only policy is preserved).
    #[test]
    fn incomplete_began_contact_is_warning_only() {
        let mut a = Arbiter::new(cfg());
        let incomplete = Contact::new(1, 0, ContactState::Began); // no coordinates
        let d = a
            .frame(&f(0, 0, vec![incomplete]))
            .expect("warning-only frame is accepted");
        assert_eq!(d.lifecycle_after, Lifecycle::Idle); // no candidate
        assert!(d.events.is_empty());
        assert!(d.diagnostics.iter().any(|d| {
            d.level == DiagnosticLevel::Warning && d.code == DiagnosticCode::IncompleteNewContact
        }));
        assert_eq!(a.lifecycle(), Lifecycle::Idle);
        // A complete contact afterwards begins a normal candidate.
        let d = run(&mut a, &[f(1, 1, vec![began(2, 0, 0.0, 0.0)])]);
        assert_eq!(d[0].lifecycle_after, Lifecycle::Candidate);
    }

    /// State atomicity when an invalid frame also carries a physical-button
    /// edge: the button bit is not applied, no `ButtonDown` is produced, and
    /// no state/baseline changes.
    #[test]
    fn invalid_frame_with_button_edge_leaves_state_and_buttons_untouched() {
        let mut a = Arbiter::new(cfg());
        run(&mut a, &[f(0, 0, vec![began(1, 0, 0.0, 0.0)])]);
        // An invalid frame (negative live tracking id) that also presses the
        // physical left button.
        let bad = frm(
            1,
            1,
            vec![Contact::new(-1, 0, ContactState::Active)],
            true,
            false,
        );
        let err = a.frame(&bad).unwrap_err();
        assert!(matches!(
            err,
            ArbiterError::InvalidFrame {
                sequence: 1,
                ref codes,
                ..
            } if codes.contains(&DiagnosticCode::InvalidEventOrder)
        ));
        // Nothing was applied: no button held, no event, lifecycle unchanged,
        // baseline unchanged (a frame with the same sequence is still
        // accepted without a regression error).
        assert!(!a.is_left_held());
        assert_eq!(a.lifecycle(), Lifecycle::Candidate);
        assert_eq!(a.tracking_id(), Some(1));
        assert_eq!(a.remainder_px(), (0.0, 0.0));
        // The next valid frame continues normally (baseline was not advanced).
        let d = run(
            &mut a,
            &[frm(2, 2, vec![active(1, 0, 2.0, 0.0)], true, false)],
        );
        assert_eq!(d[0].events, vec![down(), move_event(20.0, 0.0)]);
        assert!(a.is_left_held());
    }

    #[test]
    fn missing_coordinates_on_live_contact_cancels_and_releases_buttons() {
        let mut a = Arbiter::new(cfg());
        let mut missing = Contact::new(1, 0, ContactState::Active);
        missing.y_mm = None; // x set, y missing
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                frm(1, 1, vec![missing], true, false), // button held + missing coords
            ],
        );
        assert_eq!(d[1].lifecycle_after, Lifecycle::Cancelled);
        assert_eq!(
            d[1].transitions,
            vec![LifecycleTransition::Cancel { tracking_id: 1 }]
        );
        // The press edge is still delivered despite the cancellation.
        assert_eq!(buttons(&d), vec![down()]);
        assert!(a.is_left_held());
        // The release on a later frame is never suppressed.
        let d = run(&mut a, &[frm(2, 2, vec![], false, false)]);
        assert_eq!(buttons(&d), vec![up()]);
    }

    #[test]
    fn discontinuity_cancels_interaction_and_processes_buttons() {
        let mut a = Arbiter::new(cfg());
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 2.0, 0.0)]),
                // Discontinuity: cancel, no motion, but the release edge flows.
                frm(2, 2, vec![], false, true),
            ],
        );
        assert_eq!(d[2].lifecycle_after, Lifecycle::Cancelled);
        assert_eq!(
            d[2].transitions,
            vec![LifecycleTransition::Cancel { tracking_id: 1 }]
        );
        assert!(d[2].events.is_empty());
    }

    #[test]
    fn discontinuity_with_live_contact_starts_fresh_candidate() {
        let mut a = Arbiter::new(cfg());
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                frm(1, 1, vec![began(1, 0, 5.0, 5.0)], false, true),
            ],
        );
        assert_eq!(
            d[1].transitions,
            vec![
                LifecycleTransition::Cancel { tracking_id: 1 },
                LifecycleTransition::Begin { tracking_id: 1 },
            ]
        );
        assert_eq!(d[1].lifecycle_after, Lifecycle::Candidate);
        // The fresh candidate anchors at the resync position.
        let d = run(&mut a, &[f(2, 2, vec![active(1, 0, 5.5, 5.0)])]);
        assert_eq!(d[0].lifecycle_after, Lifecycle::Candidate); // 0.5 < 1.0
    }

    #[test]
    fn timestamp_regression_cancels_and_fails_closed() {
        let mut a = Arbiter::new(cfg());
        run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 2.0, 0.0)]),
            ],
        );
        let err = a.frame(&f(2, 0, vec![active(1, 0, 3.0, 0.0)])).unwrap_err();
        assert!(matches!(
            err,
            ArbiterError::TimestampRegression {
                found: _,
                previous: _
            }
        ));
        assert_eq!(a.lifecycle(), Lifecycle::Cancelled);
        assert_eq!(a.remainder_px(), (0.0, 0.0));
        // The baseline is retained (last accepted frame had ts 1): a frame
        // with ts 0 fails again; one with ts 2 is accepted.
        assert!(a.frame(&f(3, 0, vec![active(1, 0, 3.0, 0.0)])).is_err());
        assert!(a.frame(&f(2, 2, vec![active(1, 0, 3.0, 0.0)])).is_ok());
    }

    #[test]
    fn sequence_regression_cancels_and_fails_closed_until_release() {
        let mut a = Arbiter::new(cfg());
        run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 2.0, 0.0)]),
            ],
        );
        let err = a.frame(&f(1, 2, vec![active(1, 0, 3.0, 0.0)])).unwrap_err();
        assert!(matches!(
            err,
            ArbiterError::SequenceRegression {
                found: 1,
                previous: 1
            }
        ));
        assert_eq!(a.lifecycle(), Lifecycle::Cancelled);
        // The baseline is retained (last accepted frame had sequence 1): any
        // frame with sequence <= 1 keeps failing until release_all.
        assert!(a.frame(&f(1, 3, vec![active(1, 0, 3.0, 0.0)])).is_err());
        assert!(a.frame(&f(2, 3, vec![active(1, 0, 3.0, 0.0)])).is_ok());
        // release_all resets the regression baseline for a fresh timeline.
        assert_eq!(a.release_all(), Vec::<OutputEvent>::new());
        let d = run(&mut a, &[f(0, 0, vec![began(9, 0, 0.0, 0.0)])]);
        assert_eq!(d[0].lifecycle_after, Lifecycle::Candidate);
    }

    #[test]
    fn regression_without_active_interaction_errors_without_cancel() {
        let mut a = Arbiter::new(cfg());
        run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![ended(1, 0, 0.2, 0.0)]),
            ],
        );
        assert_eq!(a.lifecycle(), Lifecycle::Finished);
        let err = a.frame(&f(0, 2, vec![])).unwrap_err();
        assert!(matches!(err, ArbiterError::SequenceRegression { .. }));
        // Nothing active to cancel; lifecycle stays Finished.
        assert_eq!(a.lifecycle(), Lifecycle::Finished);
    }

    #[test]
    fn non_finite_arithmetic_rejects_frame_atomically() {
        // ppm = f32::MAX: any commit overflows the f32 pixel range.
        let cfg_huge =
            ArbiterConfig::new(mm(1.0), LogicalPixelsPerMm::try_new(f32::MAX).unwrap()).unwrap();
        let mut a = Arbiter::new(cfg_huge);
        run(&mut a, &[f(0, 0, vec![began(1, 0, 0.0, 0.0)])]);
        let err = a.frame(&f(1, 1, vec![active(1, 0, 2.0, 0.0)])).unwrap_err();
        assert!(matches!(err, ArbiterError::NonFinite { sequence: 1 }));
        // No partial batch and no state change: still a candidate, no moves.
        assert_eq!(a.lifecycle(), Lifecycle::Candidate);
        assert_eq!(a.remainder_px(), (0.0, 0.0));
        assert_eq!(a.tracking_id(), Some(1));
    }

    // ------------------------------------------------------------------
    // Physical buttons
    // ------------------------------------------------------------------

    #[test]
    fn click_emits_exactly_one_down_and_one_up() {
        let mut a = Arbiter::new(cfg());
        let d = run(
            &mut a,
            &[
                frm(0, 0, vec![], true, false),
                frm(1, 1, vec![], true, false),  // stable: nothing
                frm(2, 2, vec![], false, false), // release
                frm(3, 3, vec![], false, false), // stable: nothing
            ],
        );
        assert_eq!(buttons(&d), vec![down(), up()]);
        assert_eq!(d[0].events, vec![down()]);
        assert!(d[1].events.is_empty());
        assert_eq!(d[2].events, vec![up()]);
        assert!(d[3].events.is_empty());
        assert!(!a.is_left_held());
    }

    #[test]
    fn two_click_pairs_pass_through_in_order() {
        let mut a = Arbiter::new(cfg());
        let d = run(
            &mut a,
            &[
                frm(0, 0, vec![], true, false),
                frm(1, 1, vec![], false, false),
                frm(2, 2, vec![], true, false),
                frm(3, 3, vec![], false, false),
            ],
        );
        assert_eq!(buttons(&d), vec![down(), up(), down(), up()]);
    }

    #[test]
    fn press_without_contact_emits_down() {
        let mut a = Arbiter::new(cfg());
        let d = run(&mut a, &[frm(0, 0, vec![], true, false)]);
        assert_eq!(d[0].events, vec![down()]);
        assert!(a.is_left_held());
        // Releasing without a contact still emits up.
        let d = run(&mut a, &[frm(1, 1, vec![], false, false)]);
        assert_eq!(d[0].events, vec![up()]);
    }

    #[test]
    fn release_survives_cancellation_and_discontinuity() {
        let mut a = Arbiter::new(cfg());
        run(
            &mut a,
            &[
                frm(0, 0, vec![began(1, 0, 0.0, 0.0)], true, false),
                frm(1, 1, vec![active(1, 0, 2.0, 0.0)], true, false),
                // Cancellation (second contact) while still held.
                frm(
                    2,
                    2,
                    vec![active(1, 0, 3.0, 0.0), began(2, 1, 5.0, 5.0)],
                    true,
                    false,
                ),
                // Discontinuity while still held; the release edge must flow.
                frm(3, 3, vec![], false, true),
            ],
        );
        assert!(!a.is_left_held());
        let d = run(&mut a, &[frm(4, 4, vec![], false, false)]);
        assert!(d[0].events.is_empty()); // already released
    }

    #[test]
    fn same_frame_press_move_release_ordering() {
        let mut a = Arbiter::new(cfg());
        run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 2.0, 0.0)]),
            ],
        );
        // Press edge + drag movement in one frame: press precedes movement.
        let d = run(
            &mut a,
            &[frm(2, 2, vec![active(1, 0, 2.5, 0.0)], true, false)],
        );
        assert_eq!(d[0].events, vec![down(), move_event(5.0, 0.0)]);
        // Release edge + final drag movement in one frame: movement precedes
        // release.
        let d = run(
            &mut a,
            &[frm(3, 3, vec![active(1, 0, 3.0, 0.0)], false, false)],
        );
        assert_eq!(d[0].events, vec![move_event(5.0, 0.0), up()]);
        assert!(!a.is_left_held());
    }

    #[test]
    fn button_held_drag_emits_moves_between_down_and_up() {
        let mut a = Arbiter::new(cfg());
        let d = run(
            &mut a,
            &[
                frm(0, 0, vec![began(1, 0, 0.0, 0.0)], true, false), // press
                frm(1, 1, vec![active(1, 0, 2.0, 0.0)], true, false), // commit+move (drag)
                frm(2, 2, vec![active(1, 0, 3.0, 1.0)], true, false), // drag
                frm(3, 3, vec![active(1, 0, 3.0, 1.0)], false, false), // release
            ],
        );
        assert_eq!(d[0].events, vec![down()]);
        assert_eq!(d[1].events, vec![move_event(20.0, 0.0)]);
        assert_eq!(d[2].events, vec![move_event(10.0, 10.0)]);
        assert_eq!(d[3].events, vec![up()]);
        assert!(!a.is_left_held());
    }

    #[test]
    fn final_movement_precedes_release_in_ending_frame() {
        let mut a = Arbiter::new(cfg());
        let d = run(
            &mut a,
            &[
                frm(0, 0, vec![began(1, 0, 0.0, 0.0)], false, false),
                frm(1, 1, vec![active(1, 0, 2.0, 0.0)], true, false),
                // Contact ends with final coords while the button releases in
                // the same frame: final move precedes release.
                frm(2, 2, vec![ended(1, 0, 3.0, 0.0)], false, false),
            ],
        );
        assert_eq!(d[2].events, vec![move_event(10.0, 0.0), up()]);
    }

    #[test]
    fn press_edge_arrives_during_candidate_period_without_committing() {
        let mut a = Arbiter::new(cfg());
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                frm(1, 1, vec![active(1, 0, 0.5, 0.0)], true, false),
            ],
        );
        // Physical press is emitted, but the candidate stays a candidate (no
        // synthetic commit and no PointerMove from the press).
        assert_eq!(d[1].events, vec![down()]);
        assert_eq!(d[1].lifecycle_after, Lifecycle::Candidate);
    }

    // ------------------------------------------------------------------
    // release_all / shutdown
    // ------------------------------------------------------------------

    #[test]
    fn release_all_releases_held_button_exactly_once_and_resets() {
        let mut a = Arbiter::new(cfg());
        run(
            &mut a,
            &[frm(0, 0, vec![began(1, 0, 0.0, 0.0)], true, false)],
        );
        assert!(a.is_left_held());
        assert_eq!(a.release_all(), vec![up()]);
        assert!(!a.is_left_held());
        assert_eq!(a.lifecycle(), Lifecycle::Idle);
        assert_eq!(a.remainder_px(), (0.0, 0.0));
        // Idempotent: the second call emits nothing and changes nothing.
        assert_eq!(a.release_all(), Vec::<OutputEvent>::new());
    }

    #[test]
    fn release_all_clears_candidate_and_residue_even_after_errors() {
        let mut a = Arbiter::new(cfg());
        run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 2.09, 0.0)]),
            ],
        );
        // Force a regression error first (cancels the interaction).
        assert!(a.frame(&f(1, 2, vec![])).is_err());
        assert_eq!(a.lifecycle(), Lifecycle::Cancelled);
        assert_eq!(a.release_all(), Vec::<OutputEvent>::new());
        assert_eq!(a.lifecycle(), Lifecycle::Idle);
        // A fresh interaction begins cleanly after the reset.
        let d = run(&mut a, &[f(0, 0, vec![began(5, 0, 0.0, 0.0)])]);
        assert_eq!(d[0].lifecycle_after, Lifecycle::Candidate);
    }

    // ------------------------------------------------------------------
    // Sink adapter
    // ------------------------------------------------------------------

    #[test]
    fn arbiter_sink_submits_events_in_order() {
        let mut adapter = ArbiterSink::new(cfg(), RecordingSink::new());
        run_adapter(
            &mut adapter,
            &[
                frm(0, 0, vec![began(1, 0, 0.0, 0.0)], true, false), // [down]
                frm(1, 1, vec![active(1, 0, 2.0, 0.0)], true, false), // [move 20,0]
                frm(2, 2, vec![active(1, 0, 2.5, 0.0)], true, false), // [move 5,0]
                frm(3, 3, vec![active(1, 0, 2.5, 0.0)], false, false), // [up]
            ],
        );
        let events = adapter.sink().events().to_vec();
        assert_eq!(
            events,
            vec![down(), move_event(20.0, 0.0), move_event(5.0, 0.0), up()]
        );
        // release_all after everything was released emits nothing.
        let before = adapter.sink().len();
        adapter.release_all().unwrap();
        assert_eq!(adapter.sink().len(), before);
    }

    fn run_adapter(adapter: &mut ArbiterSink<RecordingSink>, frames: &[ContactFrame]) {
        for frame in frames {
            adapter.frame(frame).expect("frame must be accepted");
        }
    }

    #[derive(Default)]
    struct FrameRecordingSink {
        frames: Vec<Vec<OutputEvent>>,
        frame_timestamps: Vec<Monotonic>,
        submit_count: usize,
        reject_submit: Option<usize>,
        held_left: bool,
    }

    impl FrameRecordingSink {
        fn rejecting(submit_index: usize) -> Self {
            Self {
                reject_submit: Some(submit_index),
                ..Self::default()
            }
        }
    }

    impl OutputSink for FrameRecordingSink {
        fn submit(&mut self, event: OutputEvent) -> Result<(), OutputError> {
            let index = self.submit_count;
            self.submit_count += 1;
            if self.reject_submit == Some(index) {
                return Err(OutputError::Rejected(event));
            }
            match event {
                OutputEvent::ButtonDown(MouseButton::Left) => self.held_left = true,
                OutputEvent::ButtonUp(MouseButton::Left) => self.held_left = false,
                _ => {}
            }
            Ok(())
        }

        fn submit_frame(
            &mut self,
            events: &[OutputEvent],
        ) -> Result<(), crate::output::OutputFrameError> {
            let mut accepted = Vec::new();
            for (index, event) in events.iter().enumerate() {
                if let Err(primary) = self.submit(event.clone()) {
                    if !accepted.is_empty() {
                        self.frames.push(accepted);
                    }
                    return Err(crate::output::OutputFrameError {
                        failed_index: index,
                        accepted_prefix: index,
                        primary,
                    });
                }
                accepted.push(event.clone());
            }
            self.frames.push(accepted);
            Ok(())
        }

        fn submit_frame_at(
            &mut self,
            timestamp: Monotonic,
            events: &[OutputEvent],
        ) -> Result<(), crate::output::OutputFrameError> {
            self.frame_timestamps.push(timestamp);
            self.submit_frame(events)
        }

        fn release_all(&mut self) -> Result<(), OutputError> {
            self.held_left = false;
            Ok(())
        }
    }

    fn m19_three(state: ContactState, x: f32) -> Vec<Contact> {
        vec![
            complete(10, 0, state, x, 10.0),
            complete(11, 1, state, x + 5.0, 10.0),
            complete(12, 2, state, x + 10.0, 10.0),
        ]
    }

    #[test]
    fn arbiter_sink_forwards_source_frame_timestamp() {
        let mut adapter = ArbiterSink::new(cfg(), FrameRecordingSink::default());
        adapter
            .frame(&f(0, 100, vec![began(1, 0, 0.0, 0.0)]))
            .unwrap();
        adapter
            .frame(&f(1, 123_456_789, vec![active(1, 0, 1.0, 0.0)]))
            .unwrap();

        assert_eq!(
            adapter.sink().frame_timestamps,
            vec![Monotonic::from_nanos(123_456_789)]
        );
    }

    #[test]
    fn m19_stable_three_finger_first_motion_commits_press_before_motion_frame() {
        let config = crate::M19Profile::new(crate::UserSettings::default())
            .unwrap()
            .arbiter_config()
            .unwrap();
        let mut adapter = ArbiterSink::new(config, FrameRecordingSink::default());

        adapter
            .frame(&f(0, 0, m19_three(ContactState::Began, 0.0)))
            .unwrap();
        adapter
            .frame(&f(1, 50_000_000, m19_three(ContactState::Active, 0.4)))
            .unwrap();
        adapter
            .frame(&f(2, 60_000_000, m19_three(ContactState::Active, 1.6)))
            .unwrap();
        adapter
            .frame(&f(3, 70_000_000, m19_three(ContactState::Active, 2.2)))
            .unwrap();

        assert!(adapter.sink().frames.windows(2).any(|pair| {
            matches!(
                pair,
                [down_frame, move_frame]
                    if matches!(down_frame.as_slice(), [OutputEvent::ButtonDown(MouseButton::Left)])
                        && matches!(move_frame.as_slice(), [OutputEvent::PointerMove { .. }])
            )
        }));
    }

    #[test]
    fn m19_three_finger_tap_emits_one_middle_click_pair() {
        let config = crate::M19Profile::new(crate::UserSettings::default())
            .unwrap()
            .arbiter_config()
            .unwrap();
        let mut adapter = ArbiterSink::new(config, FrameRecordingSink::default());

        adapter
            .frame(&f(0, 0, m19_three(ContactState::Began, 0.0)))
            .unwrap();
        adapter
            .frame(&f(1, 50_000_000, m19_three(ContactState::Active, 0.0)))
            .unwrap();
        adapter.frame(&f(2, 80_000_000, vec![])).unwrap();

        assert!(adapter.sink().frames.iter().any(|frame| {
            matches!(
                frame.as_slice(),
                [
                    OutputEvent::ButtonDown(MouseButton::Middle),
                    OutputEvent::ButtonUp(MouseButton::Middle)
                ]
            )
        }));
        let middle_edges = adapter
            .sink()
            .frames
            .iter()
            .flatten()
            .filter(|event| {
                matches!(
                    event,
                    OutputEvent::ButtonDown(MouseButton::Middle)
                        | OutputEvent::ButtonUp(MouseButton::Middle)
                )
            })
            .count();
        assert_eq!(middle_edges, 2);
    }

    #[test]
    fn m19_committed_three_finger_drag_never_emits_middle_click() {
        let config = crate::M19Profile::new(crate::UserSettings::default())
            .unwrap()
            .arbiter_config()
            .unwrap();
        let mut adapter = ArbiterSink::new(config, FrameRecordingSink::default());

        adapter
            .frame(&f(0, 0, m19_three(ContactState::Began, 0.0)))
            .unwrap();
        adapter
            .frame(&f(1, 50_000_000, m19_three(ContactState::Active, 0.4)))
            .unwrap();
        adapter
            .frame(&f(2, 60_000_000, m19_three(ContactState::Active, 1.6)))
            .unwrap();
        adapter
            .frame(&f(3, 70_000_000, m19_three(ContactState::Active, 2.2)))
            .unwrap();
        adapter.frame(&f(4, 90_000_000, vec![])).unwrap();

        assert!(adapter.sink().frames.iter().flatten().all(|event| {
            !matches!(
                event,
                OutputEvent::ButtonDown(MouseButton::Middle)
                    | OutputEvent::ButtonUp(MouseButton::Middle)
            )
        }));
    }

    #[test]
    fn drag_lock_unlock_tap_releases_left_without_middle_click() {
        let config = crate::M15Profile::new()
            .unwrap()
            .arbiter_config()
            .with_gesture_bindings(crate::GestureMapConfig::default());
        let mut adapter = ArbiterSink::new(config, FrameRecordingSink::default());

        adapter
            .frame(&f(0, 0, m19_three(ContactState::Began, 0.0)))
            .unwrap();
        adapter
            .frame(&f(1, 10_000_000, m19_three(ContactState::Active, 1.2)))
            .unwrap();
        adapter.frame(&f(2, 20_000_000, vec![])).unwrap();
        adapter
            .frame(&f(3, 30_000_000, m19_three(ContactState::Began, 2.0)))
            .unwrap();
        adapter.frame(&f(4, 60_000_000, vec![])).unwrap();

        assert!(adapter
            .sink()
            .frames
            .iter()
            .flatten()
            .any(|event| { matches!(event, OutputEvent::ButtonUp(MouseButton::Left)) }));
        assert!(adapter.sink().frames.iter().flatten().all(|event| {
            !matches!(
                event,
                OutputEvent::ButtonDown(MouseButton::Middle)
                    | OutputEvent::ButtonUp(MouseButton::Middle)
            )
        }));
    }

    #[test]
    fn m19_one_finger_tap_drag_reuses_deferred_press_before_motion() {
        let config = crate::M19Profile::new(crate::UserSettings::default())
            .unwrap()
            .arbiter_config()
            .unwrap();
        let mut adapter = ArbiterSink::new(config, FrameRecordingSink::default());

        adapter
            .frame(&f(0, 0, vec![began(1, 0, 0.0, 0.0)]))
            .unwrap();
        adapter
            .frame(&f(1, 80_000_000, vec![ended(1, 0, 0.1, 0.0)]))
            .unwrap();
        adapter
            .frame(&f(2, 140_000_000, vec![began(2, 0, 10.0, 10.0)]))
            .unwrap();
        adapter
            .frame(&f(3, 160_000_000, vec![active(2, 0, 12.0, 10.0)]))
            .unwrap();

        let down_index = adapter
            .sink()
            .frames
            .iter()
            .position(|frame| {
                matches!(
                    frame.as_slice(),
                    [OutputEvent::ButtonDown(MouseButton::Left)]
                )
            })
            .expect("first tap must submit the deferred press");
        let move_index = adapter
            .sink()
            .frames
            .iter()
            .position(|frame| matches!(frame.as_slice(), [OutputEvent::PointerMove { .. }]))
            .expect("follow-up motion must be submitted");
        assert!(
            down_index < move_index,
            "deferred press must precede drag motion"
        );
    }

    #[test]
    fn split_drag_start_motion_failure_reports_global_prefix_and_keeps_left_owed() {
        let config = crate::M19Profile::new(crate::UserSettings::default())
            .unwrap()
            .arbiter_config()
            .unwrap();
        let mut adapter = ArbiterSink::new(config, FrameRecordingSink::rejecting(1));

        adapter
            .frame(&f(0, 0, m19_three(ContactState::Began, 0.0)))
            .unwrap();
        adapter
            .frame(&f(1, 50_000_000, m19_three(ContactState::Active, 0.4)))
            .unwrap();
        let err = adapter
            .frame(&f(2, 60_000_000, m19_three(ContactState::Active, 1.6)))
            .unwrap_err();

        assert!(matches!(
            err,
            ArbiterSinkError::PartialSubmit {
                index: 1,
                accepted_prefix: 1,
                decision_len: 2,
                ..
            }
        ));
        assert!(adapter.is_faulted());
        assert!(adapter.arbiter().is_left_held());
    }

    /// A scripted fault-injecting sink with a **real held-state model**:
    /// rejects specific submission indices, records accepted events, and can
    /// fail its own cleanup a configured number of times before succeeding.
    ///
    /// The held-state model mirrors the `OutputSink` contract: an accepted
    /// `ButtonDown(Left)`/`ButtonDown(Right)` sets held, the matching
    /// `ButtonUp` clears held, an accepted `ScrollBegin` opens the scroll
    /// lifecycle, `ScrollEnd` closes it, and a *successful* wrapped
    /// `release_all()` clears all held/open state (it releases all held
    /// button/key state). A rejected submit never changes the model. This
    /// lets the cleanup tests assert exactly what the sink holds after each
    /// attempt.
    struct ScriptedSink {
        events: Vec<OutputEvent>,
        reject_submits: Vec<usize>,
        submits: usize,
        release_failures_left: usize,
        releases: usize,
        /// Whether this sink itself currently holds the left button.
        held_left: bool,
        /// Whether this sink itself currently holds the right button (M9).
        held_right: bool,
        /// Whether this sink itself currently has an open scroll lifecycle
        /// (M9).
        scroll_open: bool,
    }

    impl ScriptedSink {
        fn new(reject_submits: Vec<usize>) -> Self {
            Self {
                events: Vec::new(),
                reject_submits,
                submits: 0,
                release_failures_left: 0,
                releases: 0,
                held_left: false,
                held_right: false,
                scroll_open: false,
            }
        }

        fn with_release_failures(mut self, failures: usize) -> Self {
            self.release_failures_left = failures;
            self
        }
    }

    impl OutputSink for ScriptedSink {
        fn submit(&mut self, event: OutputEvent) -> Result<(), crate::output::OutputError> {
            let index = self.submits;
            self.submits += 1;
            if self.reject_submits.contains(&index) {
                return Err(crate::output::OutputError::Rejected(event));
            }
            // A rejected submit never changes held state; an accepted event
            // updates the real held-state model.
            match &event {
                OutputEvent::ButtonDown(MouseButton::Left) => self.held_left = true,
                OutputEvent::ButtonUp(MouseButton::Left) => self.held_left = false,
                OutputEvent::ButtonDown(MouseButton::Right) => self.held_right = true,
                OutputEvent::ButtonUp(MouseButton::Right) => self.held_right = false,
                OutputEvent::ScrollBegin => self.scroll_open = true,
                OutputEvent::ScrollEnd => self.scroll_open = false,
                _ => {}
            }
            self.events.push(event);
            Ok(())
        }

        fn release_all(&mut self) -> Result<(), crate::output::OutputError> {
            self.releases += 1;
            if self.release_failures_left > 0 {
                self.release_failures_left -= 1;
                return Err(crate::output::OutputError::Io(
                    "scripted sink release_all failure".to_string(),
                ));
            }
            // A successful wrapped cleanup releases all held state.
            self.held_left = false;
            self.held_right = false;
            self.scroll_open = false;
            Ok(())
        }
    }

    /// A rejected `ButtonDown` is **not** treated as delivered: it is not
    /// tracked as held, cleanup emits no unmatched up, and recovery resets
    /// the adapter for a fresh interaction.
    #[test]
    fn rejected_first_down_is_not_held_and_causes_no_unmatched_up() {
        // The sink rejects the very first event (the ButtonDown).
        let mut adapter = ArbiterSink::new(cfg(), ScriptedSink::new(vec![0]));
        let err = adapter
            .frame(&frm(0, 0, vec![began(1, 0, 0.0, 0.0)], true, false))
            .unwrap_err();
        assert!(matches!(
            err,
            ArbiterSinkError::PartialSubmit {
                index: 0,
                accepted_prefix: 0,
                decision_len: 1,
                ..
            }
        ));
        // The rejected down is NOT tracked as held (no unmatched up possible).
        assert!(!adapter.arbiter().is_left_held());
        assert!(adapter.is_faulted());
        // Normal frames are blocked while faulted.
        let err = adapter
            .frame(&frm(1, 1, vec![active(1, 0, 2.0, 0.0)], true, false))
            .unwrap_err();
        assert!(matches!(err, ArbiterSinkError::Faulted));
        // Cleanup submits nothing (the sink never accepted a down) and resets.
        adapter.release_all().unwrap();
        assert!(!adapter.is_faulted());
        let (arbiter, sink) = adapter.into_parts();
        assert_eq!(arbiter.lifecycle(), Lifecycle::Idle);
        assert_eq!(sink.events, Vec::<OutputEvent>::new()); // no unmatched up
        assert_eq!(sink.submits, 1);
    }

    /// An accepted down followed by a failed drag movement stays known as
    /// delivered-held until cleanup succeeds; cleanup delivers the matching
    /// up exactly once (no duplicate down, no permanently lost release).
    #[test]
    fn failed_movement_after_accepted_down_stays_held_until_cleanup() {
        let mut adapter = ArbiterSink::new(cfg(), ScriptedSink::new(vec![2]));
        // Frame 0: candidate begin; no events.
        adapter
            .frame(&f(0, 0, vec![began(1, 0, 0.0, 0.0)]))
            .unwrap();
        // Frame 1: commit; submission 0 = move (20,0) accepted.
        adapter
            .frame(&f(1, 1, vec![active(1, 0, 2.0, 0.0)]))
            .unwrap();
        // Frame 2: press + drag movement; decision [down, move 5,0]:
        // the down is accepted, the movement (decision index 1) is rejected.
        let err = adapter
            .frame(&frm(2, 2, vec![active(1, 0, 2.5, 0.0)], true, false))
            .unwrap_err();
        match err {
            ArbiterSinkError::PartialSubmit {
                index,
                accepted_prefix,
                decision_len,
                failed_event,
                primary,
            } => {
                assert_eq!(index, 1);
                assert_eq!(accepted_prefix, 1);
                assert_eq!(decision_len, 2);
                assert_eq!(failed_event, move_event(5.0, 0.0));
                assert!(matches!(primary, OutputError::Rejected(_)));
            }
            other => panic!("expected PartialSubmit, got {other:?}"),
        }
        // The accepted down stays delivered-held; the adapter is faulted.
        assert!(adapter.arbiter().is_left_held());
        assert!(adapter.is_faulted());
        // Cleanup submits exactly one up for the accepted down.
        adapter.release_all().unwrap();
        let (arbiter, sink) = adapter.into_parts();
        assert_eq!(arbiter.lifecycle(), Lifecycle::Idle);
        assert_eq!(sink.events, vec![move_event(20.0, 0.0), down(), up()]);
    }

    /// An accepted down followed by a rejected up stays held; the first
    /// cleanup attempt retries the release.
    #[test]
    fn failed_up_after_accepted_down_remains_held_and_retries() {
        let mut adapter = ArbiterSink::new(cfg(), ScriptedSink::new(vec![1]));
        // Frame 0: [down] accepted (submission 0).
        adapter.frame(&frm(0, 0, vec![], true, false)).unwrap();
        // Frame 1: [up] rejected (submission 1) -> the down stays held.
        let err = adapter.frame(&frm(1, 1, vec![], false, false)).unwrap_err();
        assert!(matches!(
            err,
            ArbiterSinkError::PartialSubmit {
                index: 0,
                accepted_prefix: 0,
                decision_len: 1,
                ..
            }
        ));
        assert!(adapter.arbiter().is_left_held());
        assert!(adapter.sink().held_left);
        assert!(adapter.is_faulted());
        // Cleanup retries the up exactly once.
        adapter.release_all().unwrap();
        let (arbiter, sink) = adapter.into_parts();
        assert_eq!(arbiter.lifecycle(), Lifecycle::Idle);
        assert_eq!(sink.events, vec![down(), up()]);
        assert!(!sink.held_left);
    }

    /// R3 cleanup matrix, quadrant 1 (healthy adapter): the explicit up
    /// submission fails but the wrapped sink's own cleanup succeeds. The
    /// wrapped cleanup is **authoritative** — a successful `release_all()`
    /// released all held state — so the delivery knowledge is reconciled to
    /// released and the explicit failure is reported; a later recovery call
    /// must NOT submit another up.
    #[test]
    fn cleanup_explicit_up_fails_wrapped_succeeds_reconciles_released() {
        // Submission 0 (down) accepted; submission 1 (the first cleanup up)
        // is rejected; the wrapped release_all succeeds immediately.
        let mut adapter = ArbiterSink::new(cfg(), ScriptedSink::new(vec![1]));
        adapter.frame(&frm(0, 0, vec![], true, false)).unwrap();
        assert!(adapter.sink().held_left);
        assert!(adapter.arbiter().is_left_held());
        let err = adapter.release_all().unwrap_err();
        assert!(matches!(
            err,
            ArbiterSinkError::ReleaseFailed {
                primary: Some(OutputError::Rejected(_)),
                cleanup: None,
                ..
            }
        ));
        // The wrapped cleanup succeeded, so the sink holds nothing and the
        // adapter's delivery knowledge is reconciled to released: neither the
        // sink nor the arbiter is left-held, and no owed release remains.
        assert!(!adapter.sink().held_left);
        assert!(!adapter.arbiter().is_left_held());
        assert!(!adapter.is_faulted());
        // Recovery: the second cleanup must NOT submit another up (the wrapped
        // cleanup already released everything) — it only re-acknowledges and
        // resets the arbiter.
        adapter.release_all().unwrap();
        let (arbiter, sink) = adapter.into_parts();
        assert_eq!(arbiter.lifecycle(), Lifecycle::Idle);
        assert_eq!(sink.events, vec![down()]); // the explicit up was rejected
        assert_eq!(sink.submits, 2); // down + rejected up; no second up attempt
        assert_eq!(sink.releases, 2);
        assert!(!sink.held_left);
    }

    /// R3 cleanup matrix, quadrant 1 on an adapter already faulted by a
    /// partial frame submission: the explicit up fails while the wrapped
    /// cleanup succeeds. The wrapped cleanup is still authoritative — the
    /// release is reconciled — and the adapter stays faulted (frames blocked)
    /// until the recovery `release_all`, which must not re-submit an up.
    #[test]
    fn cleanup_explicit_up_fails_wrapped_succeeds_on_faulted_adapter() {
        // Submissions: 0 = commit move accepted, 1 = down accepted,
        // 2 = drag move rejected (PartialSubmit), 3 = first cleanup up
        // rejected; the wrapped release_all succeeds.
        let mut adapter = ArbiterSink::new(cfg(), ScriptedSink::new(vec![2, 3]));
        adapter
            .frame(&f(0, 0, vec![began(1, 0, 0.0, 0.0)]))
            .unwrap();
        adapter
            .frame(&f(1, 1, vec![active(1, 0, 2.0, 0.0)]))
            .unwrap(); // submission 0: move (20,0)
        let err = adapter
            .frame(&frm(2, 2, vec![active(1, 0, 2.5, 0.0)], true, false))
            .unwrap_err(); // [down, move 5,0]: down accepted, move rejected
        assert!(matches!(
            err,
            ArbiterSinkError::PartialSubmit {
                index: 1,
                accepted_prefix: 1,
                decision_len: 2,
                ..
            }
        ));
        assert!(adapter.arbiter().is_left_held());
        assert!(adapter.is_faulted());
        assert!(adapter.sink().held_left);
        // Cleanup: the explicit up (submission 3) is rejected but the wrapped
        // cleanup succeeds — authoritative. Held state is reconciled to
        // released; the adapter remains faulted until the recovery call.
        let err = adapter.release_all().unwrap_err();
        assert!(matches!(
            err,
            ArbiterSinkError::ReleaseFailed {
                primary: Some(OutputError::Rejected(_)),
                cleanup: None,
                ..
            }
        ));
        assert!(!adapter.sink().held_left);
        assert!(!adapter.arbiter().is_left_held());
        assert!(adapter.is_faulted()); // frames still blocked
        assert!(matches!(
            adapter.frame(&f(3, 3, vec![active(1, 0, 3.0, 0.0)])),
            Err(ArbiterSinkError::Faulted)
        ));
        // Recovery: no explicit up is re-submitted; the wrapped cleanup is
        // re-acknowledged and the adapter resets and clears the fault.
        adapter.release_all().unwrap();
        let (arbiter, sink) = adapter.into_parts();
        assert_eq!(arbiter.lifecycle(), Lifecycle::Idle);
        assert_eq!(sink.events, vec![move_event(20.0, 0.0), down()]);
        assert_eq!(sink.submits, 4); // no second up attempt after wrapped success
        assert_eq!(sink.releases, 2);
        assert!(!sink.held_left);
    }

    /// R3 cleanup matrix, quadrant 2: the explicit up submission *and* the
    /// wrapped sink's cleanup both fail. The release stays owed: held state
    /// is retained by both the sink and the adapter, and the next cleanup
    /// retries the explicit up exactly once.
    #[test]
    fn cleanup_both_fail_retains_held_and_retries_explicit_up() {
        // Submission 0 (down) accepted; submission 1 (the first cleanup up)
        // is rejected; the first wrapped release_all also fails.
        let mut adapter =
            ArbiterSink::new(cfg(), ScriptedSink::new(vec![1]).with_release_failures(1));
        adapter.frame(&frm(0, 0, vec![], true, false)).unwrap();
        let err = adapter.release_all().unwrap_err();
        assert!(matches!(
            err,
            ArbiterSinkError::ReleaseFailed {
                primary: Some(OutputError::Rejected(_)),
                cleanup: Some(_),
                ..
            }
        ));
        // Neither acknowledgement succeeded: the down is still held by both
        // the sink and the adapter, the owed release survives, and the
        // arbiter is not reset.
        assert!(adapter.sink().held_left);
        assert!(adapter.arbiter().is_left_held());
        assert!(!adapter.is_faulted());
        // The next cleanup retries the explicit up; it is accepted and the
        // wrapped cleanup now succeeds, resetting the adapter.
        adapter.release_all().unwrap();
        let (arbiter, sink) = adapter.into_parts();
        assert_eq!(arbiter.lifecycle(), Lifecycle::Idle);
        assert_eq!(sink.events, vec![down(), up()]);
        assert_eq!(sink.submits, 3); // down + rejected up + retried up
        assert_eq!(sink.releases, 2);
        assert!(!sink.held_left);
    }

    /// R3 cleanup matrix, quadrant 4: both acknowledgements succeed. The
    /// explicit up clears the sink's held state and the adapter resets
    /// normally at the acknowledgement boundary.
    #[test]
    fn cleanup_both_succeed_reset_normally() {
        let mut adapter = ArbiterSink::new(cfg(), ScriptedSink::new(vec![usize::MAX]));
        adapter.frame(&frm(0, 0, vec![], true, false)).unwrap();
        assert!(adapter.sink().held_left);
        assert!(adapter.arbiter().is_left_held());
        adapter.release_all().unwrap();
        let (arbiter, sink) = adapter.into_parts();
        assert_eq!(arbiter.lifecycle(), Lifecycle::Idle);
        assert_eq!(sink.events, vec![down(), up()]); // accepted up clears held
        assert_eq!(sink.submits, 2);
        assert_eq!(sink.releases, 1);
        assert!(!sink.held_left);
    }

    /// R3 cleanup matrix, quadrant 3: the explicit up succeeds but the wrapped
    /// sink's own cleanup fails. Not-held state is retained — the next cleanup
    /// retries only the wrapped cleanup, never a second up.
    #[test]
    fn wrapped_sink_release_all_failure_is_reported_and_retried() {
        let mut adapter = ArbiterSink::new(
            cfg(),
            ScriptedSink::new(vec![usize::MAX]).with_release_failures(1),
        );
        adapter.frame(&frm(0, 0, vec![], true, false)).unwrap();
        // The explicit up is accepted, but the wrapped sink's own cleanup
        // fails once.
        let err = adapter.release_all().unwrap_err();
        assert!(matches!(
            err,
            ArbiterSinkError::ReleaseFailed {
                primary: None,
                cleanup: Some(_),
                ..
            }
        ));
        // The up was delivered (the sink no longer holds), but the wrapped
        // cleanup must be retried.
        assert!(!adapter.sink().held_left);
        assert!(!adapter.arbiter().is_left_held());
        assert!(!adapter.is_faulted());
        // Retry: only the wrapped cleanup is re-attempted (no second up) and
        // the adapter resets.
        adapter.release_all().unwrap();
        let (arbiter, sink) = adapter.into_parts();
        assert_eq!(arbiter.lifecycle(), Lifecycle::Idle);
        assert_eq!(sink.events, vec![down(), up()]);
        assert_eq!(sink.submits, 2); // down + up; no second up attempt
        assert_eq!(sink.releases, 2);
        assert!(!sink.held_left);
    }

    /// After a fault and a successful cleanup reset, a fresh interaction
    /// produces exactly the expected wire events with no duplicate down,
    /// unmatched up, or lost release.
    #[test]
    fn fault_recovers_then_fresh_interaction_has_no_duplicates() {
        let mut adapter = ArbiterSink::new(cfg(), ScriptedSink::new(vec![0]));
        // Rejected down -> faulted.
        adapter
            .frame(&frm(0, 0, vec![began(1, 0, 0.0, 0.0)], true, false))
            .unwrap_err();
        assert!(adapter.is_faulted());
        // Cleanup resets the adapter.
        adapter.release_all().unwrap();
        assert!(!adapter.is_faulted());
        // A fresh interaction after recovery: begin -> commit+press ->
        // release.
        adapter
            .frame(&f(1, 1, vec![began(2, 0, 0.0, 0.0)]))
            .unwrap();
        let d = adapter
            .frame(&frm(2, 2, vec![active(2, 0, 2.0, 0.0)], true, false))
            .unwrap();
        assert_eq!(d.events, vec![down(), move_event(20.0, 0.0)]);
        let d = adapter
            .frame(&frm(3, 3, vec![active(2, 0, 2.0, 0.0)], false, false))
            .unwrap();
        assert_eq!(d.events, vec![up()]);
        adapter.release_all().unwrap();
        let (_, sink) = adapter.into_parts();
        assert_eq!(sink.events, vec![down(), move_event(20.0, 0.0), up()]);
    }

    // ------------------------------------------------------------------
    // Config validation
    // ------------------------------------------------------------------

    #[test]
    fn config_rejects_non_positive_threshold() {
        for bad in [0.0, -0.1, -1.0] {
            let err = ArbiterConfig::new(mm(bad), LogicalPixelsPerMm::try_new(10.0).unwrap());
            assert_eq!(err, Err(ArbiterConfigError::NonPositiveThreshold(mm(bad))));
        }
        assert!(ArbiterConfig::new(mm(0.001), LogicalPixelsPerMm::try_new(10.0).unwrap()).is_ok());
    }

    #[test]
    fn config_rejects_non_finite_or_non_positive_scale() {
        for bad in [f32::NAN, f32::INFINITY, 0.0, -5.0] {
            assert!(LogicalPixelsPerMm::try_new(bad).is_err());
        }
    }

    #[test]
    fn frame_decision_round_trips_through_json() {
        let mut a = Arbiter::new(cfg());
        let d = run(
            &mut a,
            &[
                frm(0, 0, vec![began(1, 0, 0.0, 0.0)], true, false),
                f(1, 1, vec![active(1, 0, 2.0, 0.0)]),
            ],
        );
        for decision in d {
            let json = serde_json::to_string(&decision).unwrap();
            let decoded: FrameDecision = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, decision);
        }
    }

    // ------------------------------------------------------------------
    // M8: tap configuration
    // ------------------------------------------------------------------

    fn dur(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    /// Default M8 test tap config: tap + tap-and-drag + sticky drag lock,
    /// 500 ms tap duration, 2 mm tap movement limit, 400 ms follow-up gap.
    fn tap_cfg() -> TapConfig {
        TapConfig::new(true, true, true, dur(500), mm(2.0), dur(400)).unwrap()
    }

    fn tap_cfg_arbiter_cfg() -> ArbiterConfig {
        cfg().with_tap(tap_cfg())
    }

    fn tap_arbiter() -> Arbiter {
        Arbiter::new(tap_cfg_arbiter_cfg())
    }

    #[test]
    fn tap_config_rejects_zero_durations() {
        assert_eq!(
            TapConfig::new(true, false, false, Duration::ZERO, mm(1.0), dur(100)),
            Err(TapConfigError::ZeroDuration("max_tap_duration"))
        );
        assert_eq!(
            TapConfig::new(true, false, false, dur(100), mm(1.0), Duration::ZERO),
            Err(TapConfigError::ZeroDuration("max_tap_drag_gap"))
        );
    }

    #[test]
    fn tap_config_rejects_non_positive_movement() {
        for bad in [0.0, -0.5] {
            assert_eq!(
                TapConfig::new(true, false, false, dur(100), mm(bad), dur(100)),
                Err(TapConfigError::NonPositiveMovement(mm(bad)))
            );
        }
        // Non-finite movement cannot even construct a Millimeters value.
        assert!(Millimeters::try_new(f32::NAN).is_err());
    }

    #[test]
    fn tap_config_rejects_impossible_feature_combinations() {
        assert_eq!(
            TapConfig::new(false, true, false, dur(100), mm(1.0), dur(100)),
            Err(TapConfigError::TapAndDragRequiresTap)
        );
        assert_eq!(
            TapConfig::new(true, false, true, dur(100), mm(1.0), dur(100)),
            Err(TapConfigError::DragLockRequiresTapAndDrag)
        );
        // A fully-disabled tap config with positive limits is valid.
        assert!(TapConfig::new(false, false, false, dur(100), mm(1.0), dur(100)).is_ok());
    }

    #[test]
    fn arbiter_config_default_leaves_tapping_disabled_and_with_tap_enables() {
        let base = cfg();
        assert!(base.tap_config().is_none());
        assert!(!base.is_tap_enabled());
        let tap = tap_cfg();
        let with_tap = base.with_tap(tap.clone());
        assert_eq!(with_tap.tap_config(), Some(&tap));
        assert!(with_tap.is_tap_enabled());
        assert_eq!(with_tap.tap_config().unwrap().max_tap_duration(), dur(500));
        assert_eq!(
            with_tap.tap_config().unwrap().max_tap_movement_mm(),
            mm(2.0)
        );
        assert_eq!(with_tap.tap_config().unwrap().max_tap_drag_gap(), dur(400));
        assert!(with_tap.tap_config().unwrap().tap_enabled());
        assert!(with_tap.tap_config().unwrap().tap_and_drag_enabled());
        assert!(with_tap.tap_config().unwrap().drag_lock_enabled());
    }

    #[test]
    fn default_config_without_tap_has_no_tap_output() {
        let mut a = Arbiter::new(cfg()); // tapping disabled
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![ended(1, 0, 0.1, 0.0)]),
            ],
        );
        // All M7 below-threshold sequences remain output-free.
        assert!(d.iter().all(|d| d.events.is_empty()));
        assert_eq!(d[0].tap_drag_phase_after, TapDragPhase::Idle);
        assert_eq!(d[1].tap_drag_phase_after, TapDragPhase::Idle);
    }

    // ------------------------------------------------------------------
    // M8: single tap and click pairs
    // ------------------------------------------------------------------

    #[test]
    fn single_tap_defers_release_until_follow_up_window_expires() {
        let mut a = tap_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![ended(1, 0, 0.1, 0.0)]),
            ],
        );
        assert!(d[0].events.is_empty());
        assert_eq!(d[0].tap_drag_phase_after, TapDragPhase::FirstTapCandidate);
        assert_eq!(d[0].lifecycle_after, Lifecycle::Candidate);
        assert_eq!(d[1].events, vec![down()]);
        assert_eq!(d[1].tap_drag_phase_after, TapDragPhase::FollowUpWindow);
        assert_eq!(d[1].lifecycle_after, Lifecycle::Finished);
        assert!(a.is_left_held());
        let timeout = a.tick(Monotonic::from_nanos(400_000_002)).unwrap();
        assert_eq!(timeout.events, vec![up()]);
        assert_eq!(timeout.tap_drag_phase_after, TapDragPhase::Idle);
        assert!(!a.is_left_held());
    }

    #[test]
    fn tap_without_tap_and_drag_ends_in_finished_phase() {
        let cfg_tap_only = cfg()
            .with_tap(TapConfig::new(true, false, false, dur(500), mm(2.0), dur(400)).unwrap());
        let mut a = Arbiter::new(cfg_tap_only);
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![ended(1, 0, 0.1, 0.0)]),
            ],
        );
        assert_eq!(buttons(&d), vec![down(), up()]);
        assert_eq!(d[1].tap_drag_phase_after, TapDragPhase::Finished);
    }

    #[test]
    fn two_quick_taps_emit_two_click_pairs() {
        let mut a = tap_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![ended(1, 0, 0.1, 0.0)]), // first tap: pending down
                f(2, 2, vec![began(2, 0, 1.0, 1.0)]), // follow-up pending: no button
                f(3, 3, vec![ended(2, 0, 1.0, 1.0)]), // close old click, pend new press
            ],
        );
        assert_eq!(buttons(&d), vec![down(), up(), down()]);
        assert!(d[2].events.is_empty());
        assert_eq!(d[2].tap_drag_phase_after, TapDragPhase::TapDragCandidate);
        assert_eq!(d[3].events, vec![up(), down()]);
        assert_eq!(d[3].tap_drag_phase_after, TapDragPhase::FollowUpWindow);
        let timeout = a.tick(Monotonic::from_nanos(400_000_004)).unwrap();
        assert_eq!(timeout.events, vec![up()]);
    }

    #[test]
    fn second_tap_after_window_expiry_is_ordinary_click_pair() {
        let mut a = tap_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![ended(1, 0, 0.1, 0.0)]), // first tap
                // gap strictly greater than 400 ms: the window expired
                f(2, 400_000_002, vec![began(2, 0, 1.0, 1.0)]),
                f(3, 400_000_003, vec![ended(2, 0, 1.0, 1.0)]),
            ],
        );
        assert_eq!(buttons(&d), vec![down(), up(), down()]);
        // Expiry releases the first pending click before the second contact
        // starts a fresh tap candidate.
        assert_eq!(d[2].events, vec![up()]);
        assert_eq!(d[2].tap_drag_phase_after, TapDragPhase::FirstTapCandidate);
        assert_eq!(d[3].events, vec![down()]);
        let timeout = a.tick(Monotonic::from_nanos(800_000_004)).unwrap();
        assert_eq!(timeout.events, vec![up()]);
    }

    #[test]
    fn tap_duration_boundary_equality_accepted_strictly_greater_cancels() {
        // Equality: duration exactly max_tap_duration (500 ms) is a tap.
        let mut a = tap_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 500_000_000, vec![ended(1, 0, 0.1, 0.0)]),
            ],
        );
        assert_eq!(buttons(&d), vec![down()]);
        let timeout = a.tick(Monotonic::from_nanos(900_000_001)).unwrap();
        assert_eq!(timeout.events, vec![up()]);

        // Strictly greater: no tap, no synthetic click.
        let mut a = tap_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 500_000_001, vec![ended(1, 0, 0.1, 0.0)]),
            ],
        );
        assert!(buttons(&d).is_empty());
        assert_eq!(d[1].tap_drag_phase_after, TapDragPhase::Finished);
    }

    #[test]
    fn tap_movement_boundary_equality_accepted_strictly_greater_cancels() {
        // Motion threshold 3 mm, tap limit 2 mm: the movement boundary is
        // observable without the pointer committing first.
        let cfg_wide = ArbiterConfig::new(mm(3.0), LogicalPixelsPerMm::try_new(10.0).unwrap())
            .unwrap()
            .with_tap(TapConfig::new(true, false, false, dur(500), mm(2.0), dur(400)).unwrap());
        // Equality: displacement exactly 2 mm is a tap.
        let mut a = Arbiter::new(cfg_wide.clone());
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![ended(1, 0, 2.0, 0.0)]),
            ],
        );
        assert_eq!(buttons(&d), vec![down(), up()]);
        // Strictly greater (but below the 3 mm motion threshold): no tap.
        let mut a = Arbiter::new(cfg_wide);
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![ended(1, 0, 2.5, 0.0)]),
            ],
        );
        assert!(d.iter().all(|d| d.events.is_empty()));
        assert_eq!(d[1].tap_drag_phase_after, TapDragPhase::Finished);
    }

    #[test]
    fn anchor_return_cannot_become_tap_after_exceeding_max_displacement() {
        // Motion threshold 3 mm, tap limit 1 mm.
        let cfg_wide = ArbiterConfig::new(mm(3.0), LogicalPixelsPerMm::try_new(10.0).unwrap())
            .unwrap()
            .with_tap(TapConfig::new(true, false, false, dur(500), mm(1.0), dur(400)).unwrap());
        let mut a = Arbiter::new(cfg_wide);
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 1.5, 0.0)]), // exceeds tap limit
                f(2, 2, vec![active(1, 0, 0.2, 0.0)]), // returns near the anchor
                f(3, 3, vec![ended(1, 0, 0.2, 0.0)]),
            ],
        );
        // Maximum displacement from the anchor (1.5 mm) permanently
        // disqualifies the tap even though the contact returned: no click and
        // no pointer output (below the 3 mm motion threshold).
        assert!(d.iter().all(|d| d.events.is_empty()));
        assert_eq!(d[3].tap_drag_phase_after, TapDragPhase::Finished);
    }

    #[test]
    fn tap_cancelled_by_second_finger_emits_nothing() {
        let mut a = tap_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.2, 0.0), began(2, 1, 5.0, 5.0)]),
                f(2, 2, vec![ended(1, 0, 0.2, 0.0)]),
            ],
        );
        assert!(buttons(&d).is_empty());
        assert_eq!(d[1].tap_drag_phase_after, TapDragPhase::Cancelled);
        assert_eq!(d[1].lifecycle_after, Lifecycle::Cancelled);
    }

    #[test]
    fn tap_cancelled_by_physical_press_emits_no_synthetic_click() {
        let mut a = tap_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                frm(1, 1, vec![active(1, 0, 0.2, 0.0)], true, false), // press
                frm(2, 2, vec![ended(1, 0, 0.2, 0.0)], true, false),
                frm(3, 3, vec![], false, false), // release
            ],
        );
        // Only the physical click; the tap never fires.
        assert_eq!(buttons(&d), vec![down(), up()]);
        assert_eq!(d[1].tap_drag_phase_after, TapDragPhase::Cancelled);
    }

    #[test]
    fn tap_candidate_with_physical_already_held_never_fires() {
        let mut a = tap_arbiter();
        let d = run(
            &mut a,
            &[
                frm(0, 0, vec![began(1, 0, 0.0, 0.0)], true, false), // physical held
                frm(1, 1, vec![ended(1, 0, 0.1, 0.0)], true, false),
                frm(2, 2, vec![], false, false),
            ],
        );
        assert_eq!(buttons(&d), vec![down(), up()]); // only the physical click
        assert_eq!(d[0].tap_drag_phase_after, TapDragPhase::Idle); // no tap candidate
    }

    #[test]
    fn tap_during_discontinuity_emits_nothing() {
        let mut a = tap_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                frm(1, 1, vec![ended(1, 0, 0.2, 0.0)], false, true),
            ],
        );
        assert!(buttons(&d).is_empty());
        assert_eq!(d[1].tap_drag_phase_after, TapDragPhase::Cancelled);
    }

    // ------------------------------------------------------------------
    // M8: tap-and-drag
    // ------------------------------------------------------------------

    #[test]
    fn follow_up_drag_commits_accumulated_delta_once_and_ends_with_up() {
        let cfg_no_lock =
            cfg().with_tap(TapConfig::new(true, true, false, dur(500), mm(2.0), dur(400)).unwrap());
        let mut a = Arbiter::new(cfg_no_lock);
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![ended(1, 0, 0.1, 0.0)]), // first tap: pending down
                f(2, 2, vec![began(2, 0, 10.0, 10.0)]), // inherits held left
                f(3, 3, vec![active(2, 0, 10.8, 10.0)]), // below threshold
                f(4, 4, vec![active(2, 0, 11.0, 10.0)]), // commit: 10 px once
                f(5, 5, vec![active(2, 0, 11.5, 10.0)]), // continue: 5 px
                f(6, 6, vec![ended(2, 0, 11.5, 10.0)]), // release: up
            ],
        );
        assert_eq!(buttons(&d), vec![down(), up()]);
        assert_eq!(moves(&d), vec![(10.0, 0.0), (5.0, 0.0)]);
        assert_eq!(d[4].events, vec![move_event(10.0, 0.0)]);
        assert_eq!(d[4].tap_drag_phase_after, TapDragPhase::TapDragContact);
        assert_eq!(d[6].events, vec![up()]);
        assert_eq!(d[6].tap_drag_phase_after, TapDragPhase::Finished);
    }

    #[test]
    fn synthetic_drag_final_movement_precedes_synthetic_up() {
        let cfg_no_lock =
            cfg().with_tap(TapConfig::new(true, true, false, dur(500), mm(2.0), dur(400)).unwrap());
        let mut a = Arbiter::new(cfg_no_lock);
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![ended(1, 0, 0.1, 0.0)]),
                f(2, 2, vec![began(2, 0, 10.0, 10.0)]),
                f(3, 3, vec![active(2, 0, 11.0, 10.0)]), // commit 10 px
                f(4, 4, vec![ended(2, 0, 11.5, 10.0)]),  // final move 5 px + up
            ],
        );
        assert_eq!(d[4].events, vec![move_event(5.0, 0.0), up()]);
    }

    #[test]
    fn follow_up_gap_equality_is_accepted() {
        let mut a = tap_arbiter(); // gap 400 ms
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![ended(1, 0, 0.1, 0.0)]), // completed at ts 1
                // begin exactly at the deadline (1 + 400_000_000)
                f(2, 400_000_001, vec![began(2, 0, 1.0, 1.0)]),
            ],
        );
        assert_eq!(buttons(&d), vec![down()]);
        assert!(d[2].events.is_empty());
        assert_eq!(d[2].tap_drag_phase_after, TapDragPhase::TapDragCandidate);
    }

    #[test]
    fn follow_up_window_expires_at_incoming_frame_boundary() {
        let mut a = tap_arbiter();
        run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![ended(1, 0, 0.1, 0.0)]),
            ],
        );
        assert_eq!(a.tap_drag_phase(), TapDragPhase::FollowUpWindow);
        // A frame arriving strictly after the deadline closes the window.
        let d = run(&mut a, &[f(2, 400_000_002, vec![])]);
        assert_eq!(d[0].tap_drag_phase_after, TapDragPhase::Idle);
        assert_eq!(a.tap_drag_phase(), TapDragPhase::Idle);
    }

    #[test]
    fn follow_up_requires_exactly_one_new_finger() {
        let mut a = tap_arbiter();
        run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![ended(1, 0, 0.1, 0.0)]),
            ],
        );
        // Two contacts begin inside the window: release the pending press and
        // cancel tap-drag ownership.
        let d = run(
            &mut a,
            &[f(2, 2, vec![began(2, 0, 1.0, 1.0), began(3, 1, 2.0, 2.0)])],
        );
        assert_eq!(d[0].events, vec![up()]);
        assert_eq!(d[0].tap_drag_phase_after, TapDragPhase::Cancelled);
    }

    #[test]
    fn second_finger_during_tap_drag_ends_drag_with_up() {
        let mut a = tap_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![ended(1, 0, 0.1, 0.0)]),
                f(2, 2, vec![began(2, 0, 10.0, 10.0)]), // pending, no down
                f(3, 3, vec![active(2, 0, 11.0, 10.0), began(9, 1, 5.0, 5.0)]),
            ],
        );
        assert_eq!(buttons(&d), vec![down(), up()]);
        assert!(!a.is_synthetic_left_held());
        assert_eq!(d[3].tap_drag_phase_after, TapDragPhase::Cancelled);
    }

    #[test]
    fn missing_coordinates_during_tap_drag_end_drag_with_up() {
        let mut a = tap_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![ended(1, 0, 0.1, 0.0)]),
                f(2, 2, vec![began(2, 0, 10.0, 10.0)]), // pending, no down
                f(3, 3, vec![Contact::new(2, 0, ContactState::Active)]), // missing coords
            ],
        );
        assert_eq!(buttons(&d), vec![down(), up()]);
        assert!(!a.is_synthetic_left_held());
        assert_eq!(d[3].tap_drag_phase_after, TapDragPhase::Cancelled);
    }

    // ------------------------------------------------------------------
    // M8: sticky drag lock
    // ------------------------------------------------------------------

    fn locked_arbiter() -> Arbiter {
        let mut a = tap_arbiter();
        run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![ended(1, 0, 0.1, 0.0)]),
                f(2, 2, vec![began(2, 0, 10.0, 10.0)]),
                f(3, 3, vec![active(2, 0, 11.0, 10.0)]), // commit
                f(4, 4, vec![ended(2, 0, 11.0, 10.0)]),  // lift -> locked
            ],
        );
        assert_eq!(a.tap_drag_phase(), TapDragPhase::LockedWithoutContact);
        assert!(a.is_synthetic_left_held());
        a
    }

    #[test]
    fn drag_lock_keeps_left_held_after_lift() {
        let mut a = tap_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![ended(1, 0, 0.1, 0.0)]),
                f(2, 2, vec![began(2, 0, 10.0, 10.0)]), // inherits pending down
                f(3, 3, vec![active(2, 0, 11.0, 10.0)]), // commit 10 px
                f(4, 4, vec![ended(2, 0, 11.0, 10.0)]), // lift: no up
            ],
        );
        assert_eq!(buttons(&d), vec![down()]);
        assert!(d[4].events.is_empty());
        assert_eq!(
            d[4].tap_drag_phase_after,
            TapDragPhase::LockedWithoutContact
        );
        assert!(a.is_synthetic_left_held());
        assert!(a.is_left_held());
    }

    #[test]
    fn locked_reposition_continues_drag_with_first_delta_once() {
        let mut a = locked_arbiter();
        let d = run(
            &mut a,
            &[
                f(5, 5, vec![began(3, 0, 20.0, 20.0)]), // no new down
                f(6, 6, vec![active(3, 0, 20.8, 20.0)]),
                f(7, 7, vec![active(3, 0, 21.0, 20.0)]), // commit: 10 px once
                f(8, 8, vec![active(3, 0, 21.5, 20.0)]), // continue: 5 px
                f(9, 9, vec![ended(3, 0, 21.5, 20.0)]),  // lift -> locked again
            ],
        );
        assert_eq!(moves(&d), vec![(10.0, 0.0), (5.0, 0.0)]);
        assert!(buttons(&d).is_empty()); // no button events during continuation
        assert!(a.is_synthetic_left_held());
        assert_eq!(a.tap_drag_phase(), TapDragPhase::LockedWithoutContact);
    }

    #[test]
    fn locked_qualifying_tap_unlocks_with_single_up() {
        let mut a = locked_arbiter();
        let d = run(
            &mut a,
            &[
                f(5, 5, vec![began(3, 0, 20.0, 20.0)]),
                f(6, 6, vec![ended(3, 0, 20.1, 20.0)]),
            ],
        );
        assert_eq!(buttons(&d), vec![up()]);
        assert!(!a.is_synthetic_left_held());
        assert_eq!(a.tap_drag_phase(), TapDragPhase::Idle);
    }

    #[test]
    fn locked_non_qualifying_long_contact_keeps_lock_without_click() {
        let mut a = locked_arbiter();
        // Too-long locked contact that never commits motion: no click, no up,
        // the lock stays held for another continuation attempt. The locked
        // contact begins at ts 5; ending at 500_000_006 gives a duration of
        // 500_000_001 ns, strictly greater than the 500 ms tap limit.
        let d = run(
            &mut a,
            &[
                f(5, 5, vec![began(3, 0, 20.0, 20.0)]),
                f(6, 500_000_006, vec![ended(3, 0, 20.1, 20.0)]),
            ],
        );
        assert!(buttons(&d).is_empty());
        assert!(a.is_synthetic_left_held());
        assert_eq!(a.tap_drag_phase(), TapDragPhase::LockedWithoutContact);
    }

    #[test]
    fn locked_contact_exceeding_tap_limit_keeps_lock() {
        // Motion threshold 3 mm, tap limit 1 mm, lock enabled.
        let cfg_wide = ArbiterConfig::new(mm(3.0), LogicalPixelsPerMm::try_new(10.0).unwrap())
            .unwrap()
            .with_tap(TapConfig::new(true, true, true, dur(500), mm(1.0), dur(400)).unwrap());
        let mut a = Arbiter::new(cfg_wide);
        run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![ended(1, 0, 0.1, 0.0)]),
                f(2, 2, vec![began(2, 0, 10.0, 10.0)]),
                f(3, 3, vec![active(2, 0, 13.0, 10.0)]), // commit (3 mm)
                f(4, 4, vec![ended(2, 0, 13.0, 10.0)]),  // locked
            ],
        );
        // Locked contact moves 1.5 mm (over the 1 mm tap limit, under the
        // 3 mm motion threshold) then ends: no click, no up, lock held.
        let d = run(
            &mut a,
            &[
                f(5, 5, vec![began(3, 0, 20.0, 20.0)]),
                f(6, 6, vec![active(3, 0, 21.5, 20.0)]),
                f(7, 7, vec![ended(3, 0, 21.5, 20.0)]),
            ],
        );
        assert!(buttons(&d).is_empty());
        assert!(a.is_synthetic_left_held());
        assert_eq!(a.tap_drag_phase(), TapDragPhase::LockedWithoutContact);
    }

    #[test]
    fn release_all_while_locked_emits_up_exactly_once_and_resets() {
        let mut a = locked_arbiter();
        assert_eq!(a.release_all(), vec![up()]);
        assert!(!a.is_synthetic_left_held());
        assert_eq!(a.lifecycle(), Lifecycle::Idle);
        assert_eq!(a.tap_drag_phase(), TapDragPhase::Idle);
        assert_eq!(a.remainder_px(), (0.0, 0.0));
        assert_eq!(a.release_all(), Vec::<OutputEvent>::new()); // idempotent
    }

    #[test]
    fn second_finger_during_lock_ends_lock_with_up() {
        let mut a = locked_arbiter();
        let d = run(
            &mut a,
            &[f(
                5,
                5,
                vec![began(3, 0, 20.0, 20.0), began(9, 1, 30.0, 30.0)],
            )],
        );
        assert_eq!(buttons(&d), vec![up()]);
        assert!(!a.is_synthetic_left_held());
        assert_eq!(a.tap_drag_phase(), TapDragPhase::Cancelled);
    }

    #[test]
    fn discontinuity_during_lock_ends_lock_with_up() {
        let mut a = locked_arbiter();
        let d = run(&mut a, &[frm(5, 5, vec![], false, true)]);
        assert_eq!(buttons(&d), vec![up()]);
        assert!(!a.is_synthetic_left_held());
        assert_eq!(a.tap_drag_phase(), TapDragPhase::Cancelled);
    }

    #[test]
    fn sequence_regression_during_lock_keeps_held_until_release_all() {
        let mut a = locked_arbiter();
        let err = a.frame(&f(3, 6, vec![])).unwrap_err(); // sequence regression
        assert!(matches!(err, ArbiterError::SequenceRegression { .. }));
        // Fail-closed: synthetic held state remains visible to release_all.
        assert!(a.is_synthetic_left_held());
        assert_eq!(a.tap_drag_phase(), TapDragPhase::Cancelled);
        assert_eq!(a.release_all(), vec![up()]);
        assert_eq!(a.tap_drag_phase(), TapDragPhase::Idle);
    }

    #[test]
    fn timestamp_regression_during_lock_keeps_held_until_release_all() {
        let mut a = locked_arbiter();
        // Last accepted frame had ts 4; ts 3 regresses the timestamp.
        let err = a.frame(&f(5, 3, vec![])).unwrap_err();
        assert!(matches!(err, ArbiterError::TimestampRegression { .. }));
        assert!(a.is_synthetic_left_held());
        assert_eq!(a.tap_drag_phase(), TapDragPhase::Cancelled);
        assert_eq!(a.release_all(), vec![up()]);
        assert_eq!(a.tap_drag_phase(), TapDragPhase::Idle);
        assert!(!a.is_left_held());
    }

    #[test]
    fn release_all_then_fresh_tap_interaction_works() {
        let mut a = locked_arbiter();
        assert_eq!(a.release_all(), vec![up()]);
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(9, 0, 0.0, 0.0)]),
                f(1, 1, vec![ended(9, 0, 0.1, 0.0)]),
            ],
        );
        assert_eq!(buttons(&d), vec![down()]);
        assert_eq!(a.release_all(), vec![up()]);
    }

    // ------------------------------------------------------------------
    // M8: tracking replacement
    // ------------------------------------------------------------------

    #[test]
    fn tracking_replacement_of_tap_candidate_produces_no_click() {
        let mut a = tap_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![began(2, 0, 5.0, 5.0)]), // replacement
            ],
        );
        assert!(buttons(&d).is_empty());
        // The new contact is a fresh tap candidate.
        assert_eq!(d[1].tap_drag_phase_after, TapDragPhase::FirstTapCandidate);
    }

    #[test]
    fn follow_up_tracking_bounce_cannot_turn_a_single_tap_into_drag_through() {
        let mut a = tap_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![ended(1, 0, 0.1, 0.0)]), // first tap pending press
                f(2, 2, vec![began(2, 0, 10.0, 10.0)]), // pending follow-up
                f(3, 3, vec![began(3, 0, 10.1, 10.0)]), // tracking-id bounce/replacement
                f(4, 4, vec![active(3, 0, 11.2, 10.0)]), // ordinary pointer commit
            ],
        );

        assert_eq!(d[2].tap_drag_phase_after, TapDragPhase::TapDragCandidate);
        assert!(
            d[2].events.is_empty(),
            "follow-up Began must not generate another left press"
        );
        assert_eq!(buttons(&d), vec![down(), up()]);
        assert!(moves(&d).iter().any(|(x, _)| *x > 0.0));
        assert!(!a.is_synthetic_left_held());
        assert!(!a.is_left_held());
    }

    #[test]
    fn tracking_replacement_of_drag_with_lock_continues_locked() {
        let mut a = tap_arbiter();
        run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![ended(1, 0, 0.1, 0.0)]),
                f(2, 2, vec![began(2, 0, 10.0, 10.0)]),
                f(3, 3, vec![active(2, 0, 11.0, 10.0)]), // commit
            ],
        );
        // Replacement while the synthetic drag is active: with drag lock the
        // lock continues and the new contact begins a locked continuation.
        let d = run(&mut a, &[f(4, 4, vec![began(3, 0, 20.0, 20.0)])]);
        assert!(a.is_synthetic_left_held());
        assert_eq!(a.tap_drag_phase(), TapDragPhase::LockedContact);
        assert!(d[0].events.is_empty());
        // The continuation drags normally.
        let d = run(&mut a, &[f(5, 5, vec![active(3, 0, 21.0, 20.0)])]);
        assert_eq!(moves(&d), vec![(10.0, 0.0)]);
    }

    // ------------------------------------------------------------------
    // M8: physical/synthetic left arbitration
    // ------------------------------------------------------------------

    #[test]
    fn physical_press_during_synthetic_drag_does_not_duplicate_down() {
        let mut a = tap_arbiter();
        run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![ended(1, 0, 0.1, 0.0)]),
                f(2, 2, vec![began(2, 0, 10.0, 10.0)]), // inherits pending down
                f(3, 3, vec![active(2, 0, 11.0, 10.0)]), // commit 10 px
            ],
        );
        // Physical press while the synthetic drag is active: no duplicate down.
        let d = run(
            &mut a,
            &[frm(4, 4, vec![active(2, 0, 11.5, 10.0)], true, false)],
        );
        assert_eq!(d[0].events, vec![move_event(5.0, 0.0)]);
        assert!(a.is_physical_left_held());
        assert!(!a.is_synthetic_left_held());
        // Physical left now owns the aggregate and releases it exactly once.
        let d = run(&mut a, &[frm(5, 5, vec![], false, false)]);
        assert_eq!(d[0].events, vec![up()]);
        assert!(!a.is_left_held());
    }

    #[test]
    fn synthetic_drag_end_while_physical_held_defers_up_until_physical_release() {
        let cfg_no_lock =
            cfg().with_tap(TapConfig::new(true, true, false, dur(500), mm(2.0), dur(400)).unwrap());
        let mut a = Arbiter::new(cfg_no_lock);
        run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![ended(1, 0, 0.1, 0.0)]),
                f(2, 2, vec![began(2, 0, 10.0, 10.0)]), // [down]
                f(3, 3, vec![active(2, 0, 11.0, 10.0)]), // commit
                frm(4, 4, vec![active(2, 0, 11.5, 10.0)], true, false), // press
            ],
        );
        // The drag ends while physical left holds: no synthetic up.
        let d = run(
            &mut a,
            &[frm(5, 5, vec![ended(2, 0, 11.5, 10.0)], true, false)],
        );
        assert_eq!(d[0].events, Vec::<OutputEvent>::new());
        assert!(!a.is_synthetic_left_held()); // synthetic source ended
        assert!(a.is_left_held()); // aggregate still held by physical
                                   // Physical release: the aggregate falls, exactly one up.
        let d = run(&mut a, &[frm(6, 6, vec![], false, false)]);
        assert_eq!(d[0].events, vec![up()]);
        assert!(!a.is_left_held());
    }

    #[test]
    fn aggregate_left_truth_table_physical_edges_only() {
        // (physical_prev, synthetic_prev, frame.pressed) -> expected wire
        // events, with no contact/policy activity (synthetic state unchanged).
        let cases: &[(bool, bool, bool, &[OutputEvent])] = &[
            (false, false, false, &[]),
            (false, false, true, &[down()]),
            (true, false, true, &[]), // stable held
            (true, false, false, &[up()]),
            (false, true, false, &[]), // synthetic holds: release not observable
            (false, true, true, &[]),  // synthetic holds: press not observable
            (true, true, true, &[]),   // both held, stable
            (true, true, false, &[]),  // physical release absorbed by synthetic
        ];
        for (i, &(phys_prev, synth_prev, pressed, expected)) in cases.iter().enumerate() {
            let mut a = Arbiter::new(cfg());
            // The raw physical-left state is tracked separately from the
            // unlatched source state (M9): seed both so the frame sees the
            // claimed pre-frame physical state.
            a.state.physical_left_raw = phys_prev;
            a.state.physical_left = phys_prev;
            a.state.synthetic_left = synth_prev;
            let d = a
                .frame(&frm(i as u64, i as u64, vec![], pressed, false))
                .expect("frame must be accepted");
            assert_eq!(&d.events, expected, "case {i}");
            let expected_held = pressed || synth_prev;
            assert_eq!(a.is_left_held(), expected_held, "case {i}");
        }
    }

    #[test]
    fn pointer_commit_wins_over_tap_no_click_on_release() {
        let mut a = tap_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 2.0, 0.0)]), // commit 20 px
                f(2, 2, vec![ended(1, 0, 2.0, 0.0)]),  // ends: NO tap click
            ],
        );
        assert_eq!(moves(&d), vec![(20.0, 0.0)]);
        assert!(buttons(&d).is_empty());
        assert_eq!(d[1].tap_drag_phase_after, TapDragPhase::Idle);
        assert_eq!(d[2].tap_drag_phase_after, TapDragPhase::Idle);
    }

    // ------------------------------------------------------------------
    // M8 review R1–R4 regressions
    // ------------------------------------------------------------------

    #[test]
    fn final_ended_pointer_commit_emits_no_synthetic_click() {
        // R1: a first-tap candidate that first crosses the M7 motion
        // threshold in its final Ended frame emits the pointer movement and
        // must NOT also qualify as a tap in the same release decision — even
        // when the tap movement limit (2 mm) is wider than the pointer
        // threshold (1 mm) and the final displacement sits exactly at the
        // motion threshold (equality commits). Without the ownership update
        // this would emit `PointerMove, ButtonDown, ButtonUp`.
        let mut a = tap_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.8, 0.0)]), // below threshold
                f(2, 2, vec![ended(1, 0, 1.0, 0.0)]),  // equality at threshold, final frame
            ],
        );
        assert_eq!(moves(&d), vec![(10.0, 0.0)]);
        assert!(
            buttons(&d).is_empty(),
            "pointer-only final commitment must emit no synthetic button pair"
        );
        assert_eq!(
            d[2].transitions,
            vec![
                LifecycleTransition::Commit { tracking_id: 1 },
                LifecycleTransition::Finish { tracking_id: 1 },
            ]
        );
        assert_eq!(d[2].lifecycle_after, Lifecycle::Finished);
        assert_eq!(d[2].tap_drag_phase_after, TapDragPhase::Idle);
        assert_eq!(a.tap_drag_phase(), TapDragPhase::Idle);
    }

    #[test]
    fn final_ended_tap_drag_commit_enters_lock_without_up() {
        // R1: a tap-and-drag follow-up contact that first crosses the
        // pointer threshold in its final Ended frame must mark the
        // interaction as a real drag (`drag_committed`), so with sticky drag
        // lock the lift enters locked-without-contact instead of releasing
        // an up. Final displacement sits exactly at the motion threshold
        // (equality commits).
        let mut a = tap_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![ended(1, 0, 0.1, 0.0)]), // first tap: pending down
                f(2, 2, vec![began(2, 0, 10.0, 10.0)]), // inherits held left
                f(3, 3, vec![active(2, 0, 10.8, 10.0)]), // below threshold
                f(4, 4, vec![ended(2, 0, 11.0, 10.0)]), // equality at threshold, final frame
            ],
        );
        assert_eq!(buttons(&d), vec![down()]);
        assert_eq!(moves(&d), vec![(10.0, 0.0)]);
        assert_eq!(
            d[4].tap_drag_phase_after,
            TapDragPhase::LockedWithoutContact
        );
        assert!(a.is_synthetic_left_held());
        assert!(a.is_left_held());
    }

    #[test]
    fn final_ended_locked_continuation_remains_locked() {
        // R1: a locked continuation contact that first crosses the pointer
        // threshold in its final Ended frame must remain locked
        // (`drag_committed`) and must NOT be misclassified as a qualifying
        // unlock tap — the tap movement limit (2 mm) is wider than the
        // pointer threshold (1 mm), so without the ownership update the
        // 1.0 mm final displacement would qualify as an unlock tap and emit
        // an up.
        let mut a = locked_arbiter();
        let d = run(
            &mut a,
            &[
                f(5, 5, vec![began(3, 0, 20.0, 20.0)]),
                f(6, 6, vec![active(3, 0, 20.8, 20.0)]), // below threshold
                f(7, 7, vec![ended(3, 0, 21.0, 20.0)]),  // equality at threshold, final frame
            ],
        );
        assert_eq!(moves(&d), vec![(10.0, 0.0)]);
        assert!(buttons(&d).is_empty());
        assert!(a.is_synthetic_left_held());
        assert!(a.is_left_held());
        assert_eq!(a.tap_drag_phase(), TapDragPhase::LockedWithoutContact);
    }

    #[test]
    fn discontinuity_plus_simultaneous_physical_release_emits_single_aggregate_up() {
        // R2 stateful regression (review steps 1–4): enter sticky synthetic
        // lock; press physical left while synthetic remains held (no
        // duplicate down); process one discontinuity=true frame with
        // physical left now false; require exactly one aggregate ButtonUp,
        // no panic, both sources false, lock cancelled, and repeated
        // cleanup/release producing no unmatched up.
        let mut a = locked_arbiter(); // 1. sticky synthetic lock held
        let d = run(&mut a, &[frm(5, 5, vec![], true, false)]); // 2. physical press
        assert!(
            d[0].events.is_empty(),
            "no duplicate down while synthetic holds"
        );
        assert!(a.is_physical_left_held());
        // Physical-left press explicitly takes over the aggregate tap/drag
        // ownership; the synthetic source is cleared without a wire gap.
        assert!(!a.is_synthetic_left_held());
        let d = run(&mut a, &[frm(6, 6, vec![], false, true)]); // 3. discontinuity + release
        assert_eq!(buttons(&d), vec![up()], "exactly one aggregate up"); // 4.
        assert!(!a.is_synthetic_left_held());
        assert!(!a.is_physical_left_held());
        assert!(!a.is_left_held());
        assert_eq!(a.tap_drag_phase(), TapDragPhase::Cancelled);
        // Repeated cleanup/release produces no unmatched up.
        let d = run(&mut a, &[frm(7, 7, vec![], false, false)]);
        assert!(d[0].events.is_empty());
        assert_eq!(a.release_all(), Vec::<OutputEvent>::new());
    }

    #[test]
    fn simultaneous_physical_transitions_with_synthetic_cancellation_wire_invariant() {
        // R2: for each synthetic-cancellation cause (discontinuity, extra
        // live contacts, missing active coordinates) and each combination of
        // (physical held pre-frame, frame physical state), the emitted
        // button events must leave the wire exactly in the post-frame
        // aggregate state (`is_left_held()`), in debug and release semantics
        // alike. A physical release simultaneous with the synthetic
        // cancellation must never be lost.
        //
        // Every case starts from a sticky synthetic lock (synthetic left
        // held, aggregate held) and optionally a physical press. The setup
        // must preserve the requested pre-frame physical state through to
        // the cancellation frame (review R5): the missing-coordinates cause
        // begins its locked continuation with a frame carrying `pre_phys` —
        // `f(...)` would always clear the physical source, turning the
        // `(true, false)` case into a plain synthetic end and the
        // `(true, true)` case into a physical press instead of an already
        // held physical source. Each helper asserts the actual source state
        // immediately before the cancellation frame, and every combination
        // asserts its exact expected button edge sequence plus the final
        // physical, synthetic, and aggregate states. Expected outcome is
        // identical for every cause:
        //   pre_phys=false, frame_left=false -> [up]  (synthetic end only)
        //   pre_phys=false, frame_left=true  -> []    (press absorbed by
        //                                             synthetic hold)
        //   pre_phys=true,  frame_left=false -> [up]  (physical release +
        //                                             synthetic end, one up)
        //   pre_phys=true,  frame_left=true  -> []    (physical still holds
        //                                             the wire)
        // Final synthetic is always false, physical == frame_left, aggregate
        // == frame_left, and the lock phase is Cancelled.
        fn run_discontinuity(pre_phys: bool, frame_left: bool) -> (Arbiter, Vec<OutputEvent>) {
            let mut a = locked_arbiter();
            if pre_phys {
                run(&mut a, &[frm(5, 5, vec![], true, false)]);
            }
            assert_pre_cancel_source(&a, pre_phys, "discontinuity");
            let d = run(&mut a, &[frm(6, 6, vec![], frame_left, true)]);
            (a, buttons(&d))
        }
        fn run_extra_contacts(pre_phys: bool, frame_left: bool) -> (Arbiter, Vec<OutputEvent>) {
            let mut a = locked_arbiter();
            if pre_phys {
                run(&mut a, &[frm(5, 5, vec![], true, false)]);
            }
            assert_pre_cancel_source(&a, pre_phys, "extra contacts");
            let d = run(
                &mut a,
                &[frm(
                    6,
                    6,
                    vec![began(9, 0, 5.0, 5.0), began(8, 1, 6.0, 6.0)],
                    frame_left,
                    false,
                )],
            );
            (a, buttons(&d))
        }
        fn run_missing_coords(pre_phys: bool, frame_left: bool) -> (Arbiter, Vec<OutputEvent>) {
            let mut a = locked_arbiter();
            if pre_phys {
                run(&mut a, &[frm(5, 5, vec![], true, false)]);
            }
            // Locked continuation: the setup frame must carry `pre_phys` so
            // the requested pre-frame physical state survives to the
            // cancellation frame (review R5). `f(...)` always clears the
            // physical source, which would make `(pre_phys=true, ...)`
            // release on this setup frame instead of the target frame.
            run(
                &mut a,
                &[frm(6, 6, vec![began(9, 0, 5.0, 5.0)], pre_phys, false)],
            );
            assert_pre_cancel_source(&a, pre_phys, "missing coordinates");
            let missing = Contact::new(9, 0, ContactState::Active); // no coordinates
            let d = run(&mut a, &[frm(7, 7, vec![missing], frame_left, false)]);
            (a, buttons(&d))
        }
        fn assert_pre_cancel_source(a: &Arbiter, pre_phys: bool, name: &str) {
            // A physical press now wins arbitration immediately: it takes
            // over the held aggregate and clears the synthetic owner without
            // producing a wire gap. With no physical press, the original
            // synthetic lock remains the owner until the cancellation frame.
            assert_eq!(
                a.is_physical_left_held(),
                pre_phys,
                "{name}: pre-frame physical state lost (pre_phys={pre_phys})"
            );
            assert_eq!(
                a.is_synthetic_left_held(),
                !pre_phys,
                "{name}: wrong pre-frame synthetic owner (pre_phys={pre_phys})"
            );
            assert!(
                a.is_left_held(),
                "{name}: aggregate must be held before the cancellation frame"
            );
        }
        for (name, cause) in [
            (
                "discontinuity",
                run_discontinuity as fn(bool, bool) -> (Arbiter, Vec<OutputEvent>),
            ),
            (
                "extra contacts",
                run_extra_contacts as fn(bool, bool) -> (Arbiter, Vec<OutputEvent>),
            ),
            (
                "missing coordinates",
                run_missing_coords as fn(bool, bool) -> (Arbiter, Vec<OutputEvent>),
            ),
        ] {
            for pre_phys in [false, true] {
                for frame_left in [false, true] {
                    let (a, events) = cause(pre_phys, frame_left);
                    // Exact emitted button edge sequence for this combination.
                    let expected: Vec<OutputEvent> =
                        if frame_left { Vec::new() } else { vec![up()] };
                    assert_eq!(
                        events, expected,
                        "{name}: pre_phys={pre_phys}, frame_left={frame_left}: wrong button edge sequence"
                    );
                    // Emitted wire state must equal the post-frame aggregate
                    // (synthetic left is held pre-frame in every case).
                    let mut held = pre_phys || true;
                    for e in &events {
                        match e {
                            OutputEvent::ButtonDown(MouseButton::Left) => held = true,
                            OutputEvent::ButtonUp(MouseButton::Left) => held = false,
                            _ => {}
                        }
                    }
                    assert_eq!(
                        held,
                        a.is_left_held(),
                        "{name}: pre_phys={pre_phys}, frame_left={frame_left}: wire {held} != aggregate {}",
                        a.is_left_held()
                    );
                    // Final source/aggregate states for every combination.
                    assert!(
                        !a.is_synthetic_left_held(),
                        "{name}: pre_phys={pre_phys}, frame_left={frame_left}: synthetic source must be released"
                    );
                    assert_eq!(
                        a.is_physical_left_held(),
                        frame_left,
                        "{name}: pre_phys={pre_phys}, frame_left={frame_left}: final physical state wrong"
                    );
                    assert_eq!(
                        a.is_left_held(),
                        frame_left,
                        "{name}: pre_phys={pre_phys}, frame_left={frame_left}: final aggregate state wrong"
                    );
                    assert!(
                        matches!(a.tap_drag_phase(), TapDragPhase::Cancelled | TapDragPhase::Idle),
                        "{name}: pre_phys={pre_phys}, frame_left={frame_left}: tap/lock ownership must be inactive"
                    );
                }
            }
        }
    }

    #[test]
    fn discontinuity_began_cannot_seed_tap_candidate() {
        // R3: a Began contact on a discontinuity frame must not seed the tap
        // family — a later quick/small Ended frame must not emit a click even
        // though it would qualify on duration/displacement alone. M7 pointer
        // re-anchoring still begins the candidate.
        let mut a = tap_arbiter();
        let d = run(
            &mut a,
            &[
                frm(0, 0, vec![began(1, 0, 5.0, 5.0)], false, true),
                f(1, 1, vec![ended(1, 0, 5.1, 5.0)]), // quick + small: no click
            ],
        );
        assert_eq!(d[0].tap_drag_phase_after, TapDragPhase::Idle);
        assert_eq!(d[0].lifecycle_after, Lifecycle::Candidate);
        assert!(buttons(&d).is_empty());
        assert_eq!(a.tap_drag_phase(), TapDragPhase::Idle);
    }

    #[test]
    fn discontinuity_began_preserves_m7_pointer_re_anchoring() {
        // R3: the discontinuity disqualification only blocks the tap family;
        // the M7 pointer candidate still re-anchors from the recovered frame
        // and can commit ordinary movement.
        let mut a = tap_arbiter();
        let d = run(
            &mut a,
            &[
                frm(0, 0, vec![began(1, 0, 5.0, 5.0)], false, true),
                f(1, 1, vec![active(1, 0, 5.5, 5.0)]), // below threshold: still candidate
                f(2, 2, vec![active(1, 0, 6.0, 5.0)]), // commit: 10 px pointer move
                f(3, 3, vec![ended(1, 0, 6.0, 5.0)]),  // release: pointer only, no click
            ],
        );
        assert_eq!(d[1].lifecycle_after, Lifecycle::Candidate);
        assert_eq!(moves(&d), vec![(10.0, 0.0)]);
        assert!(buttons(&d).is_empty());
        assert_eq!(a.tap_drag_phase(), TapDragPhase::Idle);
    }

    #[test]
    fn discontinuity_began_after_follow_up_window_has_no_tap_and_drag_down() {
        // R3: an open follow-up window receiving a discontinuity+Began must
        // not begin an immediate synthetic tap-and-drag down, and the
        // discontinuous contact must not click on release; a later genuinely
        // new Began after that contact ends starts tap policy normally.
        let mut a = tap_arbiter();
        run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![ended(1, 0, 0.1, 0.0)]), // first tap opens the window
            ],
        );
        assert_eq!(a.tap_drag_phase(), TapDragPhase::FollowUpWindow);
        let d = run(
            &mut a,
            &[
                frm(2, 2, vec![began(2, 0, 10.0, 10.0)], false, true), // discontinuity + Began
                f(3, 3, vec![ended(2, 0, 10.1, 10.0)]),                // quick/small: no click
            ],
        );
        assert_eq!(d[0].events, vec![up()], "pending tap press is cancelled");
        assert_eq!(d[0].tap_drag_phase_after, TapDragPhase::Idle);
        assert_eq!(buttons(&d), vec![up()]);
        // A later genuinely new Began starts tap policy normally.
        let d = run(
            &mut a,
            &[
                f(4, 4, vec![began(3, 0, 20.0, 20.0)]),
                f(5, 5, vec![ended(3, 0, 20.1, 20.0)]),
            ],
        );
        assert_eq!(d[0].tap_drag_phase_after, TapDragPhase::FirstTapCandidate);
        assert_eq!(buttons(&d), vec![down()]);
    }

    #[test]
    fn follow_up_near_u64_max_boundaries_use_checked_elapsed() {
        // R4: follow-up expiry near u64::MAX must use checked elapsed
        // duration semantics: equality with the configured gap is accepted,
        // strictly greater expires, and a nominal deadline that would
        // overflow u64::MAX is never silently converted into a different
        // state transition.
        let gap = Duration::from_nanos(500);
        let cfg_near_max =
            cfg().with_tap(TapConfig::new(true, true, true, dur(500), mm(2.0), gap).unwrap());
        let completed = u64::MAX - 1000;

        // Equality: the follow-up Began arrives exactly `gap` after the
        // completed tap -> window open, synthetic down emitted.
        let mut a = Arbiter::new(cfg_near_max.clone());
        let d = run(
            &mut a,
            &[
                f(0, u64::MAX - 1010, vec![began(1, 0, 0.0, 0.0)]),
                f(1, completed, vec![ended(1, 0, 0.1, 0.0)]), // duration 10 ns <= limit
                f(2, u64::MAX - 500, vec![began(2, 0, 10.0, 10.0)]), // elapsed == gap
            ],
        );
        assert_eq!(buttons(&d), vec![down()]);
        assert!(d[2].events.is_empty());
        assert_eq!(d[2].tap_drag_phase_after, TapDragPhase::TapDragCandidate);

        // Strictly greater: one ns past the deadline closes the window; the
        // Began becomes an ordinary candidate without a synthetic down.
        let mut a = Arbiter::new(cfg_near_max.clone());
        let d = run(
            &mut a,
            &[
                f(0, u64::MAX - 1010, vec![began(1, 0, 0.0, 0.0)]),
                f(1, completed, vec![ended(1, 0, 0.1, 0.0)]), // duration 10 ns <= limit
                f(2, u64::MAX - 499, vec![began(2, 0, 10.0, 10.0)]), // elapsed > gap
            ],
        );
        assert_eq!(buttons(&d), vec![down(), up()]);
        assert_eq!(d[2].tap_drag_phase_after, TapDragPhase::FirstTapCandidate);

        // Deadline overflow: completed + gap would exceed u64::MAX. The
        // checked elapsed comparison keeps the window open for a frame
        // within the gap (elapsed 1000 <= 2000) — the overflow is not
        // silently converted into a different state transition.
        let big_gap = Duration::from_nanos(2000);
        let cfg_overflow =
            cfg().with_tap(TapConfig::new(true, true, true, dur(500), mm(2.0), big_gap).unwrap());
        let mut a = Arbiter::new(cfg_overflow);
        let d = run(
            &mut a,
            &[
                f(0, u64::MAX - 1010, vec![began(1, 0, 0.0, 0.0)]),
                f(1, completed, vec![ended(1, 0, 0.1, 0.0)]), // duration 10 ns <= limit
                f(2, u64::MAX, vec![began(2, 0, 10.0, 10.0)]),
            ],
        );
        assert_eq!(buttons(&d), vec![down()]);
        assert!(d[2].events.is_empty());
        assert_eq!(d[2].tap_drag_phase_after, TapDragPhase::TapDragCandidate);
    }

    // ------------------------------------------------------------------
    // M8: atomicity and diagnostics
    // ------------------------------------------------------------------

    #[test]
    fn invalid_frame_does_not_change_tap_or_button_state() {
        let mut a = tap_arbiter();
        run(&mut a, &[f(0, 0, vec![began(1, 0, 0.0, 0.0)])]);
        // Invalid frame (negative live tracking id) that also presses left.
        let bad = frm(
            1,
            1,
            vec![Contact::new(-1, 0, ContactState::Active)],
            true,
            false,
        );
        assert!(a.frame(&bad).is_err());
        // Nothing changed: still a tap candidate, no button held.
        assert_eq!(a.tap_drag_phase(), TapDragPhase::FirstTapCandidate);
        assert!(!a.is_left_held());
        // The tap still enters the deferred-press window afterwards.
        let d = run(&mut a, &[f(2, 2, vec![ended(1, 0, 0.1, 0.0)])]);
        assert_eq!(buttons(&d), vec![down()]);
    }

    #[test]
    fn tap_emits_tap_fired_diagnostic() {
        let mut a = tap_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![ended(1, 0, 0.1, 0.0)]),
            ],
        );
        assert!(d[1]
            .diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::TapFired));
        assert!(d[1]
            .diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::InteractionFinished));
    }

    #[test]
    fn drag_lock_emits_lock_diagnostics() {
        let mut a = tap_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![ended(1, 0, 0.1, 0.0)]),
                f(2, 2, vec![began(2, 0, 10.0, 10.0)]),
                f(3, 3, vec![active(2, 0, 11.0, 10.0)]),
                f(4, 4, vec![ended(2, 0, 11.0, 10.0)]), // locked
            ],
        );
        assert!(d[3]
            .diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::TapAndDragBegan));
        assert!(d[4]
            .diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::DragLocked));
        // A qualifying locked tap unlocks.
        let d = run(
            &mut a,
            &[
                f(5, 5, vec![began(3, 0, 20.0, 20.0)]),
                f(6, 6, vec![ended(3, 0, 20.1, 20.0)]),
            ],
        );
        assert!(d[1]
            .diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::DragUnlocked));
    }

    // ------------------------------------------------------------------
    // M8: ArbiterSink fault handling for synthetic events
    // ------------------------------------------------------------------

    #[test]
    fn sink_rejected_tap_down_is_not_held_and_causes_no_unmatched_up() {
        let mut adapter = ArbiterSink::new(tap_cfg_arbiter_cfg(), ScriptedSink::new(vec![0]));
        adapter
            .frame(&f(0, 0, vec![began(1, 0, 0.0, 0.0)]))
            .unwrap();
        // Deferred tap press: the down (submit 0) is rejected.
        let err = adapter
            .frame(&f(1, 1, vec![ended(1, 0, 0.1, 0.0)]))
            .unwrap_err();
        assert!(matches!(
            err,
            ArbiterSinkError::PartialSubmit {
                index: 0,
                accepted_prefix: 0,
                decision_len: 1,
                ..
            }
        ));
        // The rejected tap down is NOT tracked as held (no unmatched up).
        assert!(!adapter.arbiter().is_left_held());
        assert!(adapter.is_faulted());
        adapter.release_all().unwrap();
        let (arbiter, sink) = adapter.into_parts();
        assert_eq!(arbiter.lifecycle(), Lifecycle::Idle);
        assert_eq!(sink.events, Vec::<OutputEvent>::new());
    }

    #[test]
    fn sink_rejected_tap_up_after_accepted_down_retries_on_cleanup() {
        let mut adapter = ArbiterSink::new(tap_cfg_arbiter_cfg(), ScriptedSink::new(vec![1]));
        adapter
            .frame(&f(0, 0, vec![began(1, 0, 0.0, 0.0)]))
            .unwrap();
        adapter
            .frame(&f(1, 1, vec![ended(1, 0, 0.1, 0.0)]))
            .unwrap(); // pending down = submit 0
                       // Timeout commits the pending up (submit 1), which is rejected.
        let err = adapter
            .tick(Monotonic::from_nanos(400_000_002))
            .unwrap_err();
        assert!(matches!(
            err,
            ArbiterSinkError::PartialSubmit {
                index: 0,
                accepted_prefix: 0,
                decision_len: 1,
                ..
            }
        ));
        assert!(adapter.arbiter().is_left_held());
        assert!(adapter.sink().held_left);
        assert!(adapter.is_faulted());
        // Cleanup retries the up exactly once; no duplicate down or lost
        // release.
        adapter.release_all().unwrap();
        let (arbiter, sink) = adapter.into_parts();
        assert_eq!(arbiter.lifecycle(), Lifecycle::Idle);
        assert_eq!(sink.events, vec![down(), up()]);
        assert!(!sink.held_left);
    }

    #[test]
    fn sink_motion_failure_after_accepted_synthetic_down_stays_held() {
        let mut adapter = ArbiterSink::new(tap_cfg_arbiter_cfg(), ScriptedSink::new(vec![1]));
        adapter
            .frame(&f(0, 0, vec![began(1, 0, 0.0, 0.0)]))
            .unwrap();
        // First tap: deferred down -> submission 0.
        adapter
            .frame(&f(1, 1, vec![ended(1, 0, 0.1, 0.0)]))
            .unwrap();
        // Follow-up begin is pending and emits nothing.
        adapter
            .frame(&f(2, 2, vec![began(2, 0, 10.0, 10.0)]))
            .unwrap();
        // Commit contains only move 10,0 because the original pending press
        // is already held. The move is submission 1 and is rejected.
        let err = adapter
            .frame(&f(3, 3, vec![active(2, 0, 11.0, 10.0)]))
            .unwrap_err();
        assert!(matches!(
            err,
            ArbiterSinkError::PartialSubmit {
                index: 0,
                accepted_prefix: 0,
                decision_len: 1,
                ..
            }
        ));
        // The accepted synthetic down stays delivered-held.
        assert!(adapter.arbiter().is_left_held());
        assert!(adapter.sink().held_left);
        assert!(adapter.is_faulted());
        adapter.release_all().unwrap();
        let (arbiter, sink) = adapter.into_parts();
        assert_eq!(arbiter.lifecycle(), Lifecycle::Idle);
        assert_eq!(sink.events, vec![down(), up()]);
        assert!(!sink.held_left);
    }

    #[test]
    fn sink_cleanup_while_drag_locked_releases_exactly_once() {
        let mut adapter =
            ArbiterSink::new(tap_cfg_arbiter_cfg(), ScriptedSink::new(vec![usize::MAX]));
        adapter
            .frame(&f(0, 0, vec![began(1, 0, 0.0, 0.0)]))
            .unwrap();
        adapter
            .frame(&f(1, 1, vec![ended(1, 0, 0.1, 0.0)]))
            .unwrap(); // [down, up]
        adapter
            .frame(&f(2, 2, vec![began(2, 0, 10.0, 10.0)]))
            .unwrap(); // [down]
        adapter
            .frame(&f(3, 3, vec![active(2, 0, 11.0, 10.0)]))
            .unwrap(); // [move]
        adapter
            .frame(&f(4, 4, vec![ended(2, 0, 11.0, 10.0)]))
            .unwrap(); // locked: no events
        assert!(adapter.arbiter().is_synthetic_left_held());
        assert!(adapter.arbiter().is_left_held());
        assert_eq!(
            adapter.arbiter().tap_drag_phase(),
            TapDragPhase::LockedWithoutContact
        );
        // release_all while locked: exactly one up, full reset.
        adapter.release_all().unwrap();
        let (arbiter, sink) = adapter.into_parts();
        assert_eq!(arbiter.lifecycle(), Lifecycle::Idle);
        assert_eq!(arbiter.tap_drag_phase(), TapDragPhase::Idle);
        assert_eq!(sink.events, vec![down(), move_event(10.0, 0.0), up()]);
        assert!(!sink.held_left);
    }

    #[test]
    fn sink_fault_while_locked_then_recovery_releases_once() {
        // Submissions: 0 = deferred tap down, 1 = commit move,
        // 2 = lock-cancel up (rejected).
        let mut adapter = ArbiterSink::new(tap_cfg_arbiter_cfg(), ScriptedSink::new(vec![2]));
        adapter
            .frame(&f(0, 0, vec![began(1, 0, 0.0, 0.0)]))
            .unwrap();
        adapter
            .frame(&f(1, 1, vec![ended(1, 0, 0.1, 0.0)]))
            .unwrap();
        adapter
            .frame(&f(2, 2, vec![began(2, 0, 10.0, 10.0)]))
            .unwrap();
        adapter
            .frame(&f(3, 3, vec![active(2, 0, 11.0, 10.0)]))
            .unwrap();
        adapter
            .frame(&f(4, 4, vec![ended(2, 0, 11.0, 10.0)]))
            .unwrap();
        // Second finger while locked cancels the lock with [up]; the up is
        // rejected -> PartialSubmit, delivered-held retained, adapter faulted.
        let err = adapter
            .frame(&f(5, 5, vec![began(9, 0, 5.0, 5.0), began(8, 1, 6.0, 6.0)]))
            .unwrap_err();
        assert!(matches!(
            err,
            ArbiterSinkError::PartialSubmit {
                index: 0,
                accepted_prefix: 0,
                decision_len: 1,
                ..
            }
        ));
        assert!(adapter.arbiter().is_left_held());
        assert!(adapter.is_faulted());
        // Recovery: exactly one up, then a fresh interaction.
        adapter.release_all().unwrap();
        let (arbiter, sink) = adapter.into_parts();
        assert_eq!(arbiter.lifecycle(), Lifecycle::Idle);
        assert_eq!(sink.events, vec![down(), move_event(10.0, 0.0), up()]);
        assert!(!sink.held_left);
    }

    // ------------------------------------------------------------------
    // M9: two-finger configuration
    // ------------------------------------------------------------------

    /// Default M9 test two-finger config: scroll enabled (natural), ppm 10,
    /// 0.5 mm scroll commit threshold, secondary tap enabled, buttonpad
    /// two-finger physical click enabled, 500 ms tap duration, 2 mm tap
    /// movement limit.
    fn two_cfg() -> TwoFingerConfig {
        TwoFingerConfig::new(
            true,
            true,
            LogicalPixelsPerMm::try_new(10.0).unwrap(),
            mm(0.5),
            true,
            true,
            dur(500),
            mm(2.0),
        )
        .unwrap()
    }

    fn two_arbiter_cfg() -> ArbiterConfig {
        cfg().with_two_finger(two_cfg())
    }

    fn two_arbiter() -> Arbiter {
        Arbiter::new(two_arbiter_cfg())
    }

    fn right_down() -> OutputEvent {
        OutputEvent::ButtonDown(MouseButton::Right)
    }

    fn right_up() -> OutputEvent {
        OutputEvent::ButtonUp(MouseButton::Right)
    }

    /// A frame with an arbitrary physical right-button state.
    fn frm_r(
        sequence: u64,
        ts: u64,
        contacts: Vec<Contact>,
        left: bool,
        right: bool,
    ) -> ContactFrame {
        ContactFrame {
            monotonic_timestamp: Monotonic::from_nanos(ts),
            sequence,
            discontinuity: false,
            contacts,
            physical_buttons: PhysicalButtons::new(left, right, false),
            diagnostics: vec![],
        }
    }

    /// All scroll lifecycle events across decisions, in order.
    fn scroll_events(decisions: &[FrameDecision]) -> Vec<OutputEvent> {
        decisions
            .iter()
            .flat_map(|d| d.events.iter())
            .filter(|e| {
                matches!(
                    e,
                    OutputEvent::ScrollBegin
                        | OutputEvent::ScrollDelta { .. }
                        | OutputEvent::ScrollEnd
                )
            })
            .cloned()
            .collect()
    }

    /// All ScrollDelta values across decisions, as (dx, dy).
    fn scroll_deltas(decisions: &[FrameDecision]) -> Vec<(f32, f32)> {
        decisions
            .iter()
            .flat_map(|d| d.events.iter())
            .filter_map(|e| match e {
                OutputEvent::ScrollDelta { dx, dy } => Some((dx.as_px(), dy.as_px())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn two_finger_config_disabled_by_default_and_with_two_finger_enables() {
        let base = cfg();
        assert!(base.two_finger_config().is_none());
        assert!(!base.is_two_finger_enabled());
        let two = two_cfg();
        let with_two = base.with_two_finger(two.clone());
        assert_eq!(with_two.two_finger_config(), Some(&two));
        assert!(with_two.is_two_finger_enabled());
        assert!(with_two.two_finger_config().unwrap().scroll_enabled());
        assert!(with_two.two_finger_config().unwrap().natural());
        assert_eq!(
            with_two
                .two_finger_config()
                .unwrap()
                .scroll_logical_pixels_per_mm(),
            LogicalPixelsPerMm::try_new(10.0).unwrap()
        );
        assert_eq!(
            with_two
                .two_finger_config()
                .unwrap()
                .scroll_commit_threshold_mm(),
            mm(0.5)
        );
        assert!(with_two
            .two_finger_config()
            .unwrap()
            .secondary_tap_enabled());
        assert!(with_two
            .two_finger_config()
            .unwrap()
            .two_finger_physical_click_enabled());
        assert_eq!(
            with_two
                .two_finger_config()
                .unwrap()
                .max_secondary_tap_duration(),
            dur(500)
        );
        assert_eq!(
            with_two
                .two_finger_config()
                .unwrap()
                .max_secondary_tap_movement_mm(),
            mm(2.0)
        );
    }

    #[test]
    fn two_finger_config_rejects_invalid_values() {
        // Non-positive scroll commit threshold.
        for bad in [0.0, -0.5] {
            assert_eq!(
                TwoFingerConfig::new(
                    true,
                    true,
                    LogicalPixelsPerMm::try_new(10.0).unwrap(),
                    mm(bad),
                    true,
                    true,
                    dur(100),
                    mm(1.0),
                ),
                Err(TwoFingerConfigError::NonPositiveScrollThreshold(mm(bad)))
            );
        }
        // Zero secondary-tap duration.
        assert_eq!(
            TwoFingerConfig::new(
                true,
                true,
                LogicalPixelsPerMm::try_new(10.0).unwrap(),
                mm(0.5),
                true,
                true,
                Duration::ZERO,
                mm(1.0),
            ),
            Err(TwoFingerConfigError::ZeroDuration(
                "max_secondary_tap_duration"
            ))
        );
        // Non-positive secondary-tap movement.
        assert_eq!(
            TwoFingerConfig::new(
                true,
                true,
                LogicalPixelsPerMm::try_new(10.0).unwrap(),
                mm(0.5),
                true,
                true,
                dur(100),
                mm(0.0),
            ),
            Err(TwoFingerConfigError::NonPositiveMovement(mm(0.0)))
        );
        // Non-finite scale cannot even construct a LogicalPixelsPerMm.
        assert!(LogicalPixelsPerMm::try_new(f32::NAN).is_err());
        // A fully-disabled two-finger config with valid limits is accepted.
        assert!(TwoFingerConfig::new(
            false,
            true,
            LogicalPixelsPerMm::try_new(10.0).unwrap(),
            mm(0.5),
            false,
            false,
            dur(100),
            mm(1.0),
        )
        .is_ok());
    }

    // ------------------------------------------------------------------
    // M9: two-finger candidate, threshold, and scroll
    // ------------------------------------------------------------------

    /// Frames: one finger begins, the second begins (anchor), then both move
    /// together. Returns the decisions.
    fn two_finger_scroll_run(frames: &[ContactFrame]) -> (Arbiter, Vec<FrameDecision>) {
        let mut a = two_arbiter();
        let d = run(&mut a, frames);
        (a, d)
    }

    #[test]
    fn two_finger_candidate_anchors_without_leakage_and_commits_at_threshold() {
        // Below threshold: no events leak from the candidate period.
        let (a, d) = two_finger_scroll_run(&[
            f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
            // The frame where the second valid contact appears anchors the
            // interaction: no pointer, button, or scroll event leaks.
            f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
            // Centroid moved 0.2 mm (< 0.5 threshold): still a candidate.
            f(2, 2, vec![active(1, 0, 0.2, 0.0), active(2, 1, 10.2, 0.0)]),
        ]);
        assert!(d.iter().all(|d| d.events.is_empty()));
        assert_eq!(a.two_finger_phase(), TwoFingerPhase::Candidate);
        // Equality at the threshold commits: ScrollBegin + accumulated delta
        // exactly once.
        let (a, d) = two_finger_scroll_run(&[
            f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
            f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
            // Centroid moved exactly 0.5 mm: equality commits.
            f(2, 2, vec![active(1, 0, 0.5, 0.0), active(2, 1, 10.5, 0.0)]),
        ]);
        assert_eq!(a.two_finger_phase(), TwoFingerPhase::CommittedScroll);
        assert_eq!(scroll_deltas(&d), vec![(5.0, 0.0)]);
        assert_eq!(d[2].events[0], OutputEvent::ScrollBegin);
        // Just above the threshold commits too (1.0 mm -> exactly 10 px).
        let (a, d) = two_finger_scroll_run(&[
            f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
            f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
            f(2, 2, vec![active(1, 0, 1.0, 0.0), active(2, 1, 11.0, 0.0)]),
        ]);
        assert_eq!(scroll_deltas(&d), vec![(10.0, 0.0)]);
        assert_eq!(a.scroll_remainder_px(), (0.0, 0.0));
    }

    #[test]
    fn scroll_commits_accumulated_delta_once_then_incremental() {
        // Positions are exact in binary so the scaled products are exact at
        // 10 px/mm: 0.25 mm below the 0.5 mm threshold; 1.0 mm commits the
        // accumulated 10 px exactly once; 1.5 mm increments 5 px; a zero
        // frame emits nothing; 1.0 mm back decrements 5 px.
        let (a, d) = two_finger_scroll_run(&[
            f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
            f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
            // 0.25 mm centroid movement: below the 0.5 mm threshold.
            f(
                2,
                2,
                vec![active(1, 0, 0.25, 0.0), active(2, 1, 10.25, 0.0)],
            ),
            // 1.0 mm total: commit emits the accumulated 10 px exactly once.
            f(3, 3, vec![active(1, 0, 1.0, 0.0), active(2, 1, 11.0, 0.0)]),
            // Incremental: 0.5 mm more -> 5 px.
            f(4, 4, vec![active(1, 0, 1.5, 0.0), active(2, 1, 11.5, 0.0)]),
            // Zero centroid movement -> no event.
            f(5, 5, vec![active(1, 0, 1.5, 0.0), active(2, 1, 11.5, 0.0)]),
            // Negative incremental: -0.5 mm -> -5 px.
            f(6, 6, vec![active(1, 0, 1.0, 0.0), active(2, 1, 11.0, 0.0)]),
        ]);
        assert_eq!(
            scroll_deltas(&d),
            vec![(10.0, 0.0), (5.0, 0.0), (-5.0, 0.0)]
        );
        assert_eq!(
            scroll_events(&d),
            vec![
                OutputEvent::ScrollBegin,
                OutputEvent::ScrollDelta {
                    dx: px(10.0),
                    dy: px(0.0),
                },
                OutputEvent::ScrollDelta {
                    dx: px(5.0),
                    dy: px(0.0),
                },
                OutputEvent::ScrollDelta {
                    dx: px(-5.0),
                    dy: px(0.0),
                },
            ]
        );
        assert_eq!(a.two_finger_phase(), TwoFingerPhase::CommittedScroll);
        assert!(a.is_scroll_open());
    }

    #[test]
    fn scroll_natural_direction_sign_on_both_axes() {
        // natural=true: output scroll delta keeps the centroid movement sign
        // on each axis (content follows fingers).
        let cfg_natural = cfg().with_two_finger(two_cfg());
        let frames = [
            f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
            f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
            // Centroid moves +0.8 mm in x and +0.8 mm in y.
            f(2, 2, vec![active(1, 0, 0.8, 0.8), active(2, 1, 10.8, 0.8)]),
        ];
        let mut a = Arbiter::new(cfg_natural);
        let d = run(&mut a, &frames);
        assert_eq!(scroll_deltas(&d), vec![(8.0, 8.0)]);

        // natural=false: each axis is negated.
        let cfg_non_natural = cfg().with_two_finger(
            TwoFingerConfig::new(
                true,
                false,
                LogicalPixelsPerMm::try_new(10.0).unwrap(),
                mm(0.5),
                true,
                true,
                dur(500),
                mm(2.0),
            )
            .unwrap(),
        );
        let mut a = Arbiter::new(cfg_non_natural);
        let d = run(&mut a, &frames);
        assert_eq!(scroll_deltas(&d), vec![(-8.0, -8.0)]);
    }

    #[test]
    fn scroll_axes_negative_zero_and_diagonal() {
        // x-only then y-only then diagonal then negative, all in exact 0.5 mm
        // steps so the 10 px/mm products are exact.
        let (a, d) = two_finger_scroll_run(&[
            f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
            f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
            // Commit on x-only motion: 0.5 mm -> (5, 0).
            f(2, 2, vec![active(1, 0, 0.5, 0.0), active(2, 1, 10.5, 0.0)]),
            // y-only incremental: both contacts move +0.5 mm in y -> (0, 5).
            f(3, 3, vec![active(1, 0, 0.5, 0.5), active(2, 1, 10.5, 0.5)]),
            // diagonal incremental: +0.5 mm in both axes -> (5, 5).
            f(4, 4, vec![active(1, 0, 1.0, 1.0), active(2, 1, 11.0, 1.0)]),
            // zero centroid movement: no event.
            f(5, 5, vec![active(1, 0, 1.0, 1.0), active(2, 1, 11.0, 1.0)]),
            // negative x: -0.5 mm -> (-5, 0).
            f(6, 6, vec![active(1, 0, 0.5, 1.0), active(2, 1, 10.5, 1.0)]),
        ]);
        assert_eq!(
            scroll_deltas(&d),
            vec![(5.0, 0.0), (0.0, 5.0), (5.0, 5.0), (-5.0, 0.0)]
        );
        assert_eq!(a.two_finger_phase(), TwoFingerPhase::CommittedScroll);
    }

    #[test]
    fn scroll_many_small_deltas_equal_one_aggregate_and_remainder_resets() {
        // Total centroid movement 2.0 mm per axis in exact 0.25 mm steps
        // (exact in binary, so the 10 px/mm products and the centroid
        // arithmetic carry no representation error), ppm 10 -> 20 px per
        // axis. The per-frame sub-pixel remainder must make the many-small
        // total equal the one-aggregate total.
        let cfg_small = cfg().with_two_finger(
            TwoFingerConfig::new(
                true,
                true,
                LogicalPixelsPerMm::try_new(10.0).unwrap(),
                mm(0.05),
                true,
                true,
                dur(500),
                mm(2.0),
            )
            .unwrap(),
        );
        let mut small = Arbiter::new(cfg_small.clone());
        let mut frames = vec![f(0, 0, vec![began(1, 0, 0.0, 0.0)])];
        frames.push(f(
            1,
            1,
            vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)],
        ));
        for i in 0..8 {
            let step = 0.25 * (i as f32 + 1.0);
            frames.push(f(
                i as u64 + 2,
                i as u64 + 2,
                vec![active(1, 0, step, step), active(2, 1, 10.0 + step, step)],
            ));
        }
        let d_small = run(&mut small, &frames);
        let total_small: (f32, f32) = scroll_deltas(&d_small)
            .iter()
            .fold((0.0, 0.0), |(ax, ay), (x, y)| (ax + x, ay + y));
        assert_eq!(total_small, (20.0, 20.0));

        let mut aggregate = Arbiter::new(cfg_small);
        let d_agg = run(
            &mut aggregate,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
                f(2, 2, vec![active(1, 0, 2.0, 2.0), active(2, 1, 12.0, 2.0)]),
            ],
        );
        let total_agg: (f32, f32) = scroll_deltas(&d_agg)
            .iter()
            .fold((0.0, 0.0), |(ax, ay), (x, y)| (ax + x, ay + y));
        assert_eq!(total_agg, (20.0, 20.0));
        assert_eq!(total_small, total_agg);

        // The interaction ends: the scroll remainder is reset.
        let d = run(
            &mut small,
            &[f(
                10,
                10,
                vec![ended(1, 0, 2.0, 2.0), active(2, 1, 12.0, 2.0)],
            )],
        );
        assert_eq!(scroll_events(&d), vec![OutputEvent::ScrollEnd]);
        assert_eq!(small.scroll_remainder_px(), (0.0, 0.0));
        assert_eq!(small.two_finger_phase(), TwoFingerPhase::Finished);
    }

    #[test]
    fn two_finger_pair_identity_is_independent_of_vector_order() {
        let (a, d) = two_finger_scroll_run(&[
            f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
            // Anchor: contacts listed (1, then 2).
            f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
            // The same pair listed in the opposite vector order (2, then 1):
            // the pair identity is by tracking id, not slot/vector order, so
            // the interaction continues without a replacement.
            f(2, 2, vec![active(2, 1, 10.8, 0.0), active(1, 0, 0.8, 0.0)]),
            f(3, 3, vec![active(2, 1, 11.0, 0.0), active(1, 0, 1.0, 0.0)]),
        ]);
        assert_eq!(scroll_deltas(&d), vec![(8.0, 0.0), (2.0, 0.0)]);
        assert_eq!(a.two_finger_phase(), TwoFingerPhase::CommittedScroll);
    }

    #[test]
    fn scroll_zero_delta_produces_no_event() {
        let (_, d) = two_finger_scroll_run(&[
            f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
            f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
            f(2, 2, vec![active(1, 0, 0.8, 0.0), active(2, 1, 10.8, 0.0)]),
            f(3, 3, vec![active(1, 0, 0.8, 0.0), active(2, 1, 10.8, 0.0)]),
            f(4, 4, vec![active(1, 0, 0.8, 0.0), active(2, 1, 10.8, 0.0)]),
        ]);
        // Only the commit delta; the two zero-delta frames emit nothing.
        assert_eq!(scroll_deltas(&d), vec![(8.0, 0.0)]);
    }

    // ------------------------------------------------------------------
    // M9: ending a two-finger interaction
    // ------------------------------------------------------------------

    #[test]
    fn one_finger_staggered_lift_fires_secondary_tap_at_most_once() {
        let mut a = two_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
                // First boundary that ends the exactly-two interaction: the
                // qualifying secondary tap fires exactly once.
                f(2, 2, vec![ended(1, 0, 0.0, 0.0), active(2, 1, 10.0, 0.0)]),
                // The remaining old Active contact generates no primary
                // pointer/tap output, and the tap does not fire again.
                f(3, 3, vec![active(2, 1, 10.0, 0.0)]),
                f(4, 4, vec![ended(2, 1, 10.0, 0.0)]),
            ],
        );
        assert_eq!(buttons(&d), vec![right_down(), right_up()]);
        assert_eq!(d[1].two_finger_phase_after, TwoFingerPhase::Candidate);
        assert_eq!(d[2].two_finger_phase_after, TwoFingerPhase::Finished);
        assert!(d[3].events.is_empty());
        assert!(d[4].events.is_empty());
        assert_eq!(a.two_finger_phase(), TwoFingerPhase::Finished);
        // No pointer/tap output from the remaining contact.
        assert!(moves(&d).is_empty());
    }

    #[test]
    fn both_fingers_lift_same_frame_secondary_tap_fires_once() {
        let mut a = two_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
                f(2, 2, vec![ended(1, 0, 0.0, 0.0), ended(2, 1, 10.0, 0.0)]),
            ],
        );
        assert_eq!(buttons(&d), vec![right_down(), right_up()]);
        assert_eq!(d[2].two_finger_phase_after, TwoFingerPhase::Finished);
    }

    #[test]
    fn remaining_active_contact_does_not_become_pointer_after_two_finger_end() {
        let mut a = two_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
                // One finger lifts: the two-finger interaction ends (tap
                // fires); the remaining Active contact must not silently
                // become a one-finger pointer/tap candidate without a genuine
                // new Began boundary — it may move freely with no output.
                f(2, 2, vec![ended(1, 0, 0.0, 0.0), active(2, 1, 10.0, 0.0)]),
                f(3, 3, vec![active(2, 1, 14.0, 6.0)]),
                f(4, 4, vec![ended(2, 1, 14.0, 6.0)]),
            ],
        );
        assert_eq!(buttons(&d), vec![right_down(), right_up()]);
        assert!(moves(&d).is_empty());
        // A genuinely new Began afterwards starts a normal one-finger
        // interaction.
        let d = run(
            &mut a,
            &[
                f(5, 5, vec![began(9, 0, 0.0, 0.0)]),
                f(6, 6, vec![active(9, 0, 2.0, 0.0)]),
            ],
        );
        assert_eq!(moves(&d), vec![(20.0, 0.0)]);
    }

    #[test]
    fn third_finger_ends_committed_scroll_with_scroll_end() {
        let (a, d) = two_finger_scroll_run(&[
            f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
            f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
            f(2, 2, vec![active(1, 0, 0.8, 0.0), active(2, 1, 10.8, 0.0)]),
            // A third finger appears: the interaction ends with ScrollEnd and
            // no further scroll output.
            f(
                3,
                3,
                vec![
                    active(1, 0, 0.9, 0.0),
                    active(2, 1, 10.9, 0.0),
                    began(3, 2, 20.0, 20.0),
                ],
            ),
            f(
                4,
                4,
                vec![
                    active(1, 0, 1.0, 0.0),
                    active(2, 1, 11.0, 0.0),
                    active(3, 2, 20.0, 20.0),
                ],
            ),
        ]);
        assert_eq!(
            scroll_events(&d),
            vec![
                OutputEvent::ScrollBegin,
                OutputEvent::ScrollDelta {
                    dx: px(8.0),
                    dy: px(0.0)
                },
                OutputEvent::ScrollEnd,
            ]
        );
        assert!(!a.is_scroll_open());
        assert_eq!(a.two_finger_phase(), TwoFingerPhase::Cancelled);
        // No scroll output after the end.
        assert!(d[4].events.is_empty());
    }

    #[test]
    fn missing_coordinates_end_committed_scroll_with_scroll_end() {
        let (a, d) = two_finger_scroll_run(&[
            f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
            f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
            f(2, 2, vec![active(1, 0, 0.8, 0.0), active(2, 1, 10.8, 0.0)]),
            // One contact loses its required coordinates: ScrollEnd.
            f(
                3,
                3,
                vec![
                    Contact::new(1, 0, ContactState::Active),
                    active(2, 1, 11.0, 0.0),
                ],
            ),
        ]);
        assert_eq!(
            scroll_events(&d),
            vec![
                OutputEvent::ScrollBegin,
                OutputEvent::ScrollDelta {
                    dx: px(8.0),
                    dy: px(0.0)
                },
                OutputEvent::ScrollEnd,
            ]
        );
        assert_eq!(a.two_finger_phase(), TwoFingerPhase::Cancelled);
    }

    #[test]
    fn tracking_replacement_ends_interaction_without_tap_and_no_same_frame_reanchor() {
        // Candidate phase: replacement must not fire a tap and must not
        // re-anchor in the same frame.
        let (a, d) = two_finger_scroll_run(&[
            f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
            f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
            // Tracking-id replacement: 2 -> 12 on slot 1.
            f(2, 2, vec![active(1, 0, 0.2, 0.0), began(12, 1, 10.2, 0.0)]),
        ]);
        assert!(buttons(&d).is_empty(), "replacement must not fire a tap");
        assert_eq!(a.two_finger_phase(), TwoFingerPhase::Cancelled);
        // A later stable frame with the new pair re-anchors a fresh candidate
        // (no stale anchors/remainders).
        let (a, d) = two_finger_scroll_run(&[
            f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
            f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
            f(2, 2, vec![active(1, 0, 0.2, 0.0), began(12, 1, 10.2, 0.0)]),
            // New pair (1, 12) stable: fresh candidate anchored here at the
            // centroid (5.2, 0).
            f(3, 3, vec![active(1, 0, 0.2, 0.0), active(12, 1, 10.2, 0.0)]),
            // 1.0 mm centroid movement from the fresh anchor commits; the old
            // interaction's remainder/anchors are not reused.
            f(4, 4, vec![active(1, 0, 1.2, 0.0), active(12, 1, 11.2, 0.0)]),
        ]);
        assert_eq!(scroll_deltas(&d), vec![(10.0, 0.0)]);
        assert_eq!(a.two_finger_phase(), TwoFingerPhase::CommittedScroll);
    }

    // ------------------------------------------------------------------
    // M9: secondary tap
    // ------------------------------------------------------------------

    /// Runs a two-finger tap candidate that ends at `release_ts`.
    fn secondary_tap_run(
        config: ArbiterConfig,
        anchor_ts: u64,
        release_ts: u64,
    ) -> (Arbiter, Vec<FrameDecision>) {
        let mut a = Arbiter::new(config);
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(
                    1,
                    anchor_ts,
                    vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)],
                ),
                f(
                    2,
                    release_ts,
                    vec![ended(1, 0, 0.0, 0.0), active(2, 1, 10.0, 0.0)],
                ),
            ],
        );
        (a, d)
    }

    #[test]
    fn secondary_tap_duration_boundary_equality_accepted_strictly_greater_disqualifies() {
        // Equality: duration exactly max_secondary_tap_duration (500 ms).
        let (a, d) = secondary_tap_run(two_arbiter_cfg(), 1, 500_000_001);
        assert_eq!(buttons(&d), vec![right_down(), right_up()]);
        assert_eq!(a.two_finger_phase(), TwoFingerPhase::Finished);
        // Strictly greater: no secondary click.
        let (a, d) = secondary_tap_run(two_arbiter_cfg(), 1, 500_000_002);
        assert!(buttons(&d).is_empty());
        assert_eq!(a.two_finger_phase(), TwoFingerPhase::Finished);
    }

    #[test]
    fn secondary_tap_per_contact_displacement_boundary() {
        // Scroll threshold 5 mm (never commits), tap movement limit 1 mm.
        let cfg_wide = cfg().with_two_finger(
            TwoFingerConfig::new(
                true,
                true,
                LogicalPixelsPerMm::try_new(10.0).unwrap(),
                mm(5.0),
                true,
                true,
                dur(500),
                mm(1.0),
            )
            .unwrap(),
        );
        // Equality: one contact displaced exactly 1 mm from its anchor is a
        // secondary tap (below the 5 mm scroll threshold).
        let mut a = Arbiter::new(cfg_wide.clone());
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
                f(2, 2, vec![ended(1, 0, 1.0, 0.0), active(2, 1, 10.0, 0.0)]),
            ],
        );
        assert_eq!(buttons(&d), vec![right_down(), right_up()]);
        // Strictly greater: one contact displaced 1.5 mm — no tap.
        let mut a = Arbiter::new(cfg_wide);
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
                f(2, 2, vec![ended(1, 0, 1.5, 0.0), active(2, 1, 10.0, 0.0)]),
            ],
        );
        assert!(buttons(&d).is_empty());
    }

    #[test]
    fn opposing_pinch_motion_cannot_falsely_qualify_as_secondary_tap() {
        // Scroll threshold 5 mm, tap movement limit 0.5 mm. The two contacts
        // move 1 mm apart (opposing motion): the centroid returns to its
        // anchor, so the scroll never commits, but each contact's maximum
        // displacement from its own anchor exceeds the tap limit — the
        // interaction must not qualify as a secondary tap.
        let cfg_wide = cfg().with_two_finger(
            TwoFingerConfig::new(
                true,
                true,
                LogicalPixelsPerMm::try_new(10.0).unwrap(),
                mm(5.0),
                true,
                true,
                dur(500),
                mm(0.5),
            )
            .unwrap(),
        );
        let mut a = Arbiter::new(cfg_wide);
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
                // Pinch: A moves +1 mm, B moves -1 mm -> centroid unchanged.
                f(2, 2, vec![active(1, 0, 1.0, 0.0), active(2, 1, 9.0, 0.0)]),
                // Both lift: no tap (per-contact displacement 1 mm > 0.5 mm).
                f(3, 3, vec![ended(1, 0, 1.0, 0.0), ended(2, 1, 9.0, 0.0)]),
            ],
        );
        assert!(buttons(&d).is_empty());
        assert!(scroll_deltas(&d).is_empty());
        assert_eq!(a.two_finger_phase(), TwoFingerPhase::Finished);
    }

    #[test]
    fn secondary_tap_disabled_produces_nothing() {
        let cfg_no_tap = cfg().with_two_finger(
            TwoFingerConfig::new(
                true,
                true,
                LogicalPixelsPerMm::try_new(10.0).unwrap(),
                mm(0.5),
                false, // secondary tap disabled
                true,
                dur(500),
                mm(2.0),
            )
            .unwrap(),
        );
        let (a, d) = secondary_tap_run(cfg_no_tap, 1, 2);
        assert!(buttons(&d).is_empty());
        assert_eq!(a.two_finger_phase(), TwoFingerPhase::Finished);
    }

    #[test]
    fn two_secondary_taps_produce_two_right_click_pairs() {
        let mut a = two_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
                f(2, 2, vec![ended(1, 0, 0.0, 0.0), active(2, 1, 10.0, 0.0)]),
                f(3, 3, vec![ended(2, 1, 10.0, 0.0)]),
                // Second interaction.
                f(4, 4, vec![began(3, 0, 20.0, 20.0)]),
                f(
                    5,
                    5,
                    vec![active(3, 0, 20.0, 20.0), began(4, 1, 30.0, 20.0)],
                ),
                f(
                    6,
                    6,
                    vec![ended(3, 0, 20.0, 20.0), active(4, 1, 30.0, 20.0)],
                ),
                f(7, 7, vec![ended(4, 1, 30.0, 20.0)]),
            ],
        );
        // Two ordinary right click pairs (no invented double-click event).
        assert_eq!(
            buttons(&d),
            vec![right_down(), right_up(), right_down(), right_up(),]
        );
    }

    #[test]
    fn scroll_wins_over_secondary_tap() {
        let (a, d) = two_finger_scroll_run(&[
            f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
            f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
            f(2, 2, vec![active(1, 0, 0.8, 0.0), active(2, 1, 10.8, 0.0)]),
            // Committed scroll ends by release: ScrollEnd, never a tap.
            f(3, 3, vec![ended(1, 0, 0.8, 0.0), active(2, 1, 10.8, 0.0)]),
        ]);
        assert!(buttons(&d).is_empty(), "committed scroll must not tap");
        assert_eq!(
            scroll_events(&d),
            vec![
                OutputEvent::ScrollBegin,
                OutputEvent::ScrollDelta {
                    dx: px(8.0),
                    dy: px(0.0)
                },
                OutputEvent::ScrollEnd,
            ]
        );
        assert_eq!(a.two_finger_phase(), TwoFingerPhase::Finished);
    }

    // ------------------------------------------------------------------
    // M9: family ownership
    // ------------------------------------------------------------------

    #[test]
    fn two_finger_family_wins_over_one_finger_pointer_without_double_commit() {
        let mut a = two_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 2.0, 0.0)]), // one-finger commits (20, 0)
                // The second finger appears: the one-finger interaction is
                // cancelled (no further pointer output) and the two-finger
                // family anchors the contacts.
                f(2, 2, vec![active(1, 0, 2.0, 0.0), began(2, 1, 10.0, 0.0)]),
                f(3, 3, vec![active(1, 0, 2.5, 0.0), active(2, 1, 10.5, 0.0)]),
            ],
        );
        // Only the one-finger commit (20, 0); no pointer output after the
        // second finger appeared, and the two-finger scroll committed once.
        assert_eq!(moves(&d), vec![(20.0, 0.0)]);
        assert_eq!(scroll_deltas(&d), vec![(5.0, 0.0)]);
        assert_eq!(d[2].lifecycle_after, Lifecycle::Cancelled);
        assert_eq!(d[2].two_finger_phase_after, TwoFingerPhase::Candidate);
        assert_eq!(a.two_finger_phase(), TwoFingerPhase::CommittedScroll);
    }

    #[test]
    fn two_finger_entry_releases_sticky_drag_lock_with_one_left_up() {
        // Tap + two-finger config: enter sticky drag lock, then two fingers
        // appear — the sticky synthetic-left lock is released per the M8
        // aggregate-source rules (exactly one ButtonUp(Left)) before the
        // two-finger family owns the contacts.
        let cfg_combined = cfg().with_tap(tap_cfg()).with_two_finger(two_cfg());
        let mut a = Arbiter::new(cfg_combined);
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![ended(1, 0, 0.1, 0.0)]), // first tap [down, up]
                f(2, 2, vec![began(2, 0, 10.0, 10.0)]), // synthetic [down]
                f(3, 3, vec![active(2, 0, 11.0, 10.0)]), // commit
                f(4, 4, vec![ended(2, 0, 11.0, 10.0)]), // locked: no up
                // Two fingers: the lock releases with exactly one left up and
                // the two-finger candidate anchors.
                f(5, 5, vec![began(9, 0, 20.0, 20.0), began(8, 1, 30.0, 20.0)]),
            ],
        );
        assert_eq!(buttons(&d), vec![down(), up()]);
        assert!(!a.is_left_held());
        assert_eq!(a.tap_drag_phase(), TapDragPhase::Cancelled);
        assert_eq!(a.two_finger_phase(), TwoFingerPhase::Candidate);
    }

    // ------------------------------------------------------------------
    // M9: buttonpad physical two-finger click and right-button sources
    // ------------------------------------------------------------------

    #[test]
    fn physical_two_finger_click_latches_right_down_and_up() {
        let mut a = two_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
                // Physical-left press while exactly two fingers are down:
                // latched to Right, never a Left down.
                frm(
                    2,
                    2,
                    vec![active(1, 0, 0.2, 0.0), active(2, 1, 10.2, 0.0)],
                    true,
                    false,
                ),
                // Still held: stable, nothing.
                frm(
                    3,
                    3,
                    vec![active(1, 0, 0.2, 0.0), active(2, 1, 10.2, 0.0)],
                    true,
                    false,
                ),
                // Matching physical release: exactly one ButtonUp(Right).
                frm(
                    4,
                    4,
                    vec![active(1, 0, 0.2, 0.0), active(2, 1, 10.2, 0.0)],
                    false,
                    false,
                ),
            ],
        );
        assert_eq!(buttons(&d), vec![right_down(), right_up()]);
        assert!(!a.is_left_held());
        assert!(!a.is_right_held());
        assert_eq!(
            d[2].two_finger_phase_after,
            TwoFingerPhase::PhysicalSecondaryClickHeld
        );
        assert_eq!(d[4].two_finger_phase_after, TwoFingerPhase::Finished);
    }

    #[test]
    fn one_finger_physical_click_remains_left() {
        let mut a = two_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                frm(1, 1, vec![active(1, 0, 0.0, 0.0)], true, false),
                frm(2, 2, vec![active(1, 0, 0.0, 0.0)], false, false),
            ],
        );
        assert_eq!(buttons(&d), vec![down(), up()]);
        assert!(!a.is_right_held());
    }

    #[test]
    fn press_before_second_finger_remains_left() {
        let mut a = two_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                // Press while only one finger is down: a primary-left press.
                frm(1, 1, vec![active(1, 0, 0.0, 0.0)], true, false),
                // The second finger appears while the press is held: the press
                // stays Left (no remap, no duplicate down).
                frm(
                    2,
                    2,
                    vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)],
                    true,
                    false,
                ),
                // Release: still a Left up.
                frm(
                    3,
                    3,
                    vec![active(1, 0, 0.0, 0.0), active(2, 1, 10.0, 0.0)],
                    false,
                    false,
                ),
            ],
        );
        assert_eq!(buttons(&d), vec![down(), up()]);
        assert!(!a.is_right_held());
        assert!(!a.is_left_held());
    }

    #[test]
    fn finger_count_changes_while_latched_do_not_remap() {
        let mut a = two_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
                // Latch: physical-left press with two fingers -> Right.
                frm(
                    2,
                    2,
                    vec![active(1, 0, 0.0, 0.0), active(2, 1, 10.0, 0.0)],
                    true,
                    false,
                ),
                // One finger lifts while held: the latch never remaps.
                frm(3, 3, vec![active(2, 1, 10.0, 0.0)], true, false),
                // Both lift while held.
                frm(4, 4, vec![], true, false),
                // New fingers land while still held: still Right.
                frm(
                    5,
                    5,
                    vec![began(3, 0, 20.0, 20.0), began(4, 1, 30.0, 20.0)],
                    true,
                    false,
                ),
                // Matching physical release: exactly one Right up.
                frm(
                    6,
                    6,
                    vec![active(3, 0, 20.0, 20.0), active(4, 1, 30.0, 20.0)],
                    false,
                    false,
                ),
            ],
        );
        assert_eq!(buttons(&d), vec![right_down(), right_up()]);
        assert!(!a.is_left_held());
        assert!(!a.is_right_held());
        assert!(!a.is_latched_right_held());
    }

    #[test]
    fn physical_two_finger_click_cancels_candidate_no_tap_on_release() {
        let mut a = two_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
                // The physical two-finger click cancels the candidate.
                frm(
                    2,
                    2,
                    vec![active(1, 0, 0.0, 0.0), active(2, 1, 10.0, 0.0)],
                    true,
                    false,
                ),
                frm(
                    3,
                    3,
                    vec![active(1, 0, 0.0, 0.0), active(2, 1, 10.0, 0.0)],
                    false,
                    false,
                ),
                // Fingers lift afterwards: no synthetic secondary tap on
                // release (the release frame does not re-seed a candidate).
                f(4, 4, vec![ended(1, 0, 0.0, 0.0), active(2, 1, 10.0, 0.0)]),
                f(5, 5, vec![ended(2, 1, 10.0, 0.0)]),
            ],
        );
        assert_eq!(buttons(&d), vec![right_down(), right_up()]);
        assert_eq!(a.two_finger_phase(), TwoFingerPhase::Finished);
    }

    #[test]
    fn latch_on_first_two_finger_frame_disqualifies_reanchored_candidate() {
        // The physical click begins on the very first frame with two fingers
        // (before the candidate formally anchors): the press is latched to
        // Right, and after the release the continuing contacts may re-anchor
        // for relative scroll but must not fire a secondary tap on a quick
        // lift.
        let mut a = two_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                // Second finger begins AND the physical press lands in the
                // same frame: latched to Right.
                frm(
                    1,
                    1,
                    vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)],
                    true,
                    false,
                ),
                frm(
                    2,
                    2,
                    vec![active(1, 0, 0.0, 0.0), active(2, 1, 10.0, 0.0)],
                    false,
                    false,
                ),
                // The same fingers lift right after the click: no secondary
                // tap (the re-anchored candidate is tap-disqualified).
                f(3, 3, vec![ended(1, 0, 0.0, 0.0), active(2, 1, 10.0, 0.0)]),
                f(4, 4, vec![ended(2, 1, 10.0, 0.0)]),
            ],
        );
        assert_eq!(buttons(&d), vec![right_down(), right_up()]);
        assert_eq!(a.two_finger_phase(), TwoFingerPhase::Finished);
    }

    #[test]
    fn latch_release_reanchored_contacts_stay_tap_disqualified() {
        // After a latched physical click, the same fingers may keep scrolling
        // (relative scroll re-anchors) but a quick lift must not fire a
        // secondary tap: the click's tap disqualification survives the latch
        // release and the re-anchor.
        let mut a = two_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
                // Latched physical click: RightDown.
                frm(
                    2,
                    2,
                    vec![active(1, 0, 0.0, 0.0), active(2, 1, 10.0, 0.0)],
                    true,
                    false,
                ),
                // Release: RightUp; the same frame must not re-seed a
                // candidate.
                frm(
                    3,
                    3,
                    vec![active(1, 0, 0.0, 0.0), active(2, 1, 10.0, 0.0)],
                    false,
                    false,
                ),
                // Both fingers still down one more frame: a fresh candidate
                // re-anchors (tap-disqualified by the click).
                f(4, 4, vec![active(1, 0, 0.0, 0.0), active(2, 1, 10.0, 0.0)]),
                // Quick lift: no secondary tap.
                f(5, 5, vec![ended(1, 0, 0.0, 0.0), active(2, 1, 10.0, 0.0)]),
                f(6, 6, vec![ended(2, 1, 10.0, 0.0)]),
            ],
        );
        assert_eq!(buttons(&d), vec![right_down(), right_up()]);
        assert!(scroll_deltas(&d).is_empty());
        assert_eq!(a.two_finger_phase(), TwoFingerPhase::Finished);
    }

    #[test]
    fn physical_right_edges_pass_through_the_right_aggregate() {
        let mut a = two_arbiter();
        let d = run(
            &mut a,
            &[
                frm_r(0, 0, vec![], false, true),
                frm_r(1, 1, vec![], false, true),
                frm_r(2, 2, vec![], false, false),
            ],
        );
        assert_eq!(buttons(&d), vec![right_down(), right_up()]);
        assert!(d[0].events == vec![right_down()]);
        assert!(d[1].events.is_empty());
        assert!(d[2].events == vec![right_up()]);
        assert!(!a.is_right_held());
    }

    #[test]
    fn physical_right_press_cancels_two_finger_candidate() {
        let mut a = two_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
                // A physical right press competes: the candidate is cancelled
                // (no secondary tap later) and the right source holds.
                frm_r(
                    2,
                    2,
                    vec![active(1, 0, 0.0, 0.0), active(2, 1, 10.0, 0.0)],
                    false,
                    true,
                ),
                frm_r(
                    3,
                    3,
                    vec![active(1, 0, 0.0, 0.0), active(2, 1, 10.0, 0.0)],
                    false,
                    false,
                ),
                // Fingers lift: no synthetic tap.
                f(4, 4, vec![ended(1, 0, 0.0, 0.0), active(2, 1, 10.0, 0.0)]),
            ],
        );
        assert_eq!(buttons(&d), vec![right_down(), right_up()]);
        assert_eq!(a.two_finger_phase(), TwoFingerPhase::Finished);
    }

    #[test]
    fn aggregate_right_truth_table_three_sources() {
        // (physical_prev, synthetic_prev, latch_prev, frame.right) ->
        // expected wire events, with no contact/policy activity (the
        // synthetic/latched sources stay unchanged).
        let cases: &[(bool, bool, bool, bool, &[OutputEvent])] = &[
            (false, false, false, false, &[]),
            (false, false, false, true, &[right_down()]),
            (true, false, false, true, &[]), // stable held
            (true, false, false, false, &[right_up()]),
            (false, true, false, false, &[]), // synthetic holds: release hidden
            (false, true, false, true, &[]),  // synthetic holds: press hidden
            (false, false, true, false, &[]), // latch holds: release hidden
            (false, false, true, true, &[]),  // latch holds: press hidden
            (true, true, true, true, &[]),    // all held, stable
            (true, true, true, false, &[]),   // release absorbed by synthetic/latch
        ];
        for (i, &(phys_prev, synth_prev, latch_prev, pressed, expected)) in cases.iter().enumerate()
        {
            let mut a = Arbiter::new(cfg());
            a.state.physical_right_raw = phys_prev;
            a.state.physical_right = phys_prev;
            a.state.synthetic_right = synth_prev;
            a.state.latched_right_owned = latch_prev;
            let d = a
                .frame(&frm_r(i as u64, i as u64, vec![], false, pressed))
                .expect("frame must be accepted");
            assert_eq!(&d.events, expected, "case {i}");
            let expected_held = pressed || synth_prev || latch_prev;
            assert_eq!(a.is_right_held(), expected_held, "case {i}");
        }
    }

    #[test]
    fn physical_two_finger_click_ends_committed_scroll_with_scroll_end() {
        let (a, d) = two_finger_scroll_run(&[
            f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
            f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
            f(2, 2, vec![active(1, 0, 0.8, 0.0), active(2, 1, 10.8, 0.0)]),
            // A two-finger physical click owns the interaction: the scroll
            // ends (ScrollEnd) and the press is latched to Right.
            frm(
                3,
                3,
                vec![active(1, 0, 0.9, 0.0), active(2, 1, 10.9, 0.0)],
                true,
                false,
            ),
            frm(
                4,
                4,
                vec![active(1, 0, 0.9, 0.0), active(2, 1, 10.9, 0.0)],
                false,
                false,
            ),
        ]);
        assert_eq!(
            scroll_events(&d),
            vec![
                OutputEvent::ScrollBegin,
                OutputEvent::ScrollDelta {
                    dx: px(8.0),
                    dy: px(0.0)
                },
                OutputEvent::ScrollEnd,
            ]
        );
        assert_eq!(buttons(&d), vec![right_down(), right_up()]);
        assert!(!a.is_scroll_open());
        assert_eq!(a.two_finger_phase(), TwoFingerPhase::Finished);
    }

    // ------------------------------------------------------------------
    // M9: discontinuity, regression, atomicity, release_all
    // ------------------------------------------------------------------

    #[test]
    fn discontinuity_reanchors_scroll_candidate_but_no_secondary_tap() {
        let mut a = two_arbiter();
        let d = run(
            &mut a,
            &[
                // A discontinuity frame with exactly two live contacts
                // re-anchors a fresh candidate (tap-disqualified).
                frm(
                    0,
                    0,
                    vec![began(1, 0, 5.0, 5.0), began(2, 1, 15.0, 5.0)],
                    false,
                    true,
                ),
                // Relative scroll from the fresh anchor works.
                f(1, 1, vec![active(1, 0, 5.5, 5.0), active(2, 1, 15.5, 5.0)]),
                // Ends by release: ScrollEnd, never a tap (the pair began
                // across the recovered boundary).
                f(2, 2, vec![ended(1, 0, 5.5, 5.0), active(2, 1, 15.5, 5.0)]),
            ],
        );
        assert_eq!(
            scroll_events(&d),
            vec![
                OutputEvent::ScrollBegin,
                OutputEvent::ScrollDelta {
                    dx: px(5.0),
                    dy: px(0.0)
                },
                OutputEvent::ScrollEnd,
            ]
        );
        assert!(buttons(&d).is_empty());
        assert_eq!(a.two_finger_phase(), TwoFingerPhase::Finished);
    }

    #[test]
    fn invalid_frame_leaves_two_finger_state_atomic() {
        let mut a = two_arbiter();
        run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
                // 1.0 mm commit: exactly (10, 0), remainder (0, 0).
                f(2, 2, vec![active(1, 0, 1.0, 0.0), active(2, 1, 11.0, 0.0)]),
            ],
        );
        assert!(a.is_scroll_open());
        assert_eq!(a.scroll_remainder_px(), (0.0, 0.0));
        // An invalid frame (negative live tracking id) that also presses the
        // physical left button: rejected wholesale — no state/button change.
        let bad = frm(
            3,
            3,
            vec![Contact::new(-1, 0, ContactState::Active)],
            true,
            false,
        );
        assert!(a.frame(&bad).is_err());
        assert!(a.is_scroll_open());
        assert!(!a.is_left_held());
        assert_eq!(a.two_finger_phase(), TwoFingerPhase::CommittedScroll);
        // The scroll continues normally afterwards (+0.5 mm -> (5, 0)).
        let d = run(
            &mut a,
            &[f(
                4,
                4,
                vec![active(1, 0, 1.5, 0.0), active(2, 1, 11.5, 0.0)],
            )],
        );
        assert_eq!(scroll_deltas(&d), vec![(5.0, 0.0)]);
    }

    #[test]
    fn sequence_regression_keeps_scroll_open_until_release_all() {
        let mut a = two_arbiter();
        run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
                f(2, 2, vec![active(1, 0, 0.8, 0.0), active(2, 1, 10.8, 0.0)]),
            ],
        );
        assert!(a.is_scroll_open());
        // Sequence regression: the frame is rejected but the open scroll
        // remains visible to release_all (fail-closed).
        let err = a.frame(&f(2, 3, vec![])).unwrap_err();
        assert!(matches!(err, ArbiterError::SequenceRegression { .. }));
        assert!(a.is_scroll_open());
        assert_eq!(a.two_finger_phase(), TwoFingerPhase::Cancelled);
        assert_eq!(a.release_all(), vec![OutputEvent::ScrollEnd]);
        assert_eq!(a.release_all(), Vec::<OutputEvent>::new()); // idempotent
    }

    #[test]
    fn regression_keeps_latched_right_visible_until_release_all() {
        let mut a = two_arbiter();
        run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
                frm(
                    2,
                    2,
                    vec![active(1, 0, 0.0, 0.0), active(2, 1, 10.0, 0.0)],
                    true,
                    false,
                ),
            ],
        );
        assert!(a.is_latched_right_held());
        // Timestamp regression: rejected, but the latched right stays visible.
        let err = a.frame(&frm(3, 1, vec![], true, false)).unwrap_err();
        assert!(matches!(err, ArbiterError::TimestampRegression { .. }));
        assert!(a.is_latched_right_held());
        assert_eq!(a.release_all(), vec![right_up()]);
        assert_eq!(a.release_all(), Vec::<OutputEvent>::new());
    }

    #[test]
    fn release_all_closes_scroll_and_releases_right_exactly_once() {
        // During a candidate (no scroll open, no buttons): empty.
        let mut a = two_arbiter();
        run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
            ],
        );
        assert_eq!(a.release_all(), Vec::<OutputEvent>::new());
        assert_eq!(a.two_finger_phase(), TwoFingerPhase::Idle);

        // During a committed scroll: exactly one ScrollEnd.
        let mut a = two_arbiter();
        run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
                f(2, 2, vec![active(1, 0, 0.8, 0.0), active(2, 1, 10.8, 0.0)]),
            ],
        );
        assert_eq!(a.release_all(), vec![OutputEvent::ScrollEnd]);
        assert!(!a.is_scroll_open());
        assert_eq!(a.two_finger_phase(), TwoFingerPhase::Idle);

        // While a latched physical right press is held: exactly one
        // ButtonUp(Right).
        let mut a = two_arbiter();
        run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
                frm(
                    2,
                    2,
                    vec![active(1, 0, 0.0, 0.0), active(2, 1, 10.0, 0.0)],
                    true,
                    false,
                ),
            ],
        );
        assert_eq!(a.release_all(), vec![right_up()]);
        assert!(!a.is_right_held());
        // A fresh interaction works after the reset.
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
                f(2, 2, vec![ended(1, 0, 0.0, 0.0), active(2, 1, 10.0, 0.0)]),
            ],
        );
        assert_eq!(buttons(&d), vec![right_down(), right_up()]);
    }

    #[test]
    fn two_finger_phase_is_observable() {
        let mut a = two_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
                f(2, 2, vec![active(1, 0, 0.8, 0.0), active(2, 1, 10.8, 0.0)]),
                f(3, 3, vec![ended(1, 0, 0.8, 0.0), active(2, 1, 10.8, 0.0)]),
            ],
        );
        assert_eq!(d[0].two_finger_phase_after, TwoFingerPhase::Idle);
        assert_eq!(d[1].two_finger_phase_after, TwoFingerPhase::Candidate);
        assert_eq!(d[2].two_finger_phase_after, TwoFingerPhase::CommittedScroll);
        assert_eq!(d[3].two_finger_phase_after, TwoFingerPhase::Finished);
        assert_eq!(a.two_finger_phase(), TwoFingerPhase::Finished);
    }

    #[test]
    fn frame_decision_serializes_with_two_finger_phase() {
        let mut a = two_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
                f(2, 2, vec![active(1, 0, 0.8, 0.0), active(2, 1, 10.8, 0.0)]),
            ],
        );
        for decision in d {
            let json = serde_json::to_string(&decision).unwrap();
            let decoded: FrameDecision = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, decision);
        }
    }

    // ------------------------------------------------------------------
    // M9: ArbiterSink accepted-prefix fault handling for Right and Scroll
    // ------------------------------------------------------------------

    #[test]
    fn sink_rejected_scroll_begin_owes_no_scroll_end() {
        let mut adapter = ArbiterSink::new(two_arbiter_cfg(), ScriptedSink::new(vec![0]));
        adapter
            .frame(&f(0, 0, vec![began(1, 0, 0.0, 0.0)]))
            .unwrap();
        adapter
            .frame(&f(
                1,
                1,
                vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)],
            ))
            .unwrap();
        // The commit decision [ScrollBegin, ScrollDelta]: ScrollBegin (sub 0)
        // is rejected -> no scroll lifecycle was delivered.
        let err = adapter
            .frame(&f(
                2,
                2,
                vec![active(1, 0, 0.8, 0.0), active(2, 1, 10.8, 0.0)],
            ))
            .unwrap_err();
        assert!(matches!(
            err,
            ArbiterSinkError::PartialSubmit {
                index: 0,
                accepted_prefix: 0,
                decision_len: 2,
                ..
            }
        ));
        assert!(!adapter.arbiter().is_scroll_open());
        assert!(adapter.is_faulted());
        // Cleanup submits no ScrollEnd (none was owed) and resets.
        adapter.release_all().unwrap();
        let (arbiter, sink) = adapter.into_parts();
        assert_eq!(arbiter.lifecycle(), Lifecycle::Idle);
        assert_eq!(sink.events, Vec::<OutputEvent>::new());
        assert!(!sink.scroll_open);
    }

    #[test]
    fn sink_rejected_first_delta_after_accepted_begin_cleanup_closes_scroll() {
        let mut adapter = ArbiterSink::new(two_arbiter_cfg(), ScriptedSink::new(vec![1]));
        adapter
            .frame(&f(0, 0, vec![began(1, 0, 0.0, 0.0)]))
            .unwrap();
        adapter
            .frame(&f(
                1,
                1,
                vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)],
            ))
            .unwrap();
        // ScrollBegin (sub 0) accepted, ScrollDelta (sub 1) rejected: the
        // scroll lifecycle stays open and cleanup must close it.
        let err = adapter
            .frame(&f(
                2,
                2,
                vec![active(1, 0, 0.8, 0.0), active(2, 1, 10.8, 0.0)],
            ))
            .unwrap_err();
        assert!(matches!(
            err,
            ArbiterSinkError::PartialSubmit {
                index: 1,
                accepted_prefix: 1,
                decision_len: 2,
                ..
            }
        ));
        assert!(adapter.arbiter().is_scroll_open());
        assert!(adapter.sink().scroll_open);
        assert!(adapter.is_faulted());
        adapter.release_all().unwrap();
        let (arbiter, sink) = adapter.into_parts();
        assert_eq!(arbiter.lifecycle(), Lifecycle::Idle);
        // The first delta was rejected, so only the accepted ScrollBegin and
        // the cleanup ScrollEnd reach the sink.
        assert_eq!(
            sink.events,
            vec![OutputEvent::ScrollBegin, OutputEvent::ScrollEnd]
        );
        assert!(!sink.scroll_open);
    }

    #[test]
    fn sink_rejected_scroll_end_after_accepted_begin_retries() {
        // Submissions: 0 = ScrollBegin, 1 = ScrollDelta, 2 = ScrollEnd.
        let mut adapter = ArbiterSink::new(two_arbiter_cfg(), ScriptedSink::new(vec![2]));
        adapter
            .frame(&f(0, 0, vec![began(1, 0, 0.0, 0.0)]))
            .unwrap();
        adapter
            .frame(&f(
                1,
                1,
                vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)],
            ))
            .unwrap();
        adapter
            .frame(&f(
                2,
                2,
                vec![active(1, 0, 0.8, 0.0), active(2, 1, 10.8, 0.0)],
            ))
            .unwrap();
        // End frame: [ScrollEnd] rejected -> the open lifecycle stays owed.
        let err = adapter
            .frame(&f(
                3,
                3,
                vec![ended(1, 0, 0.8, 0.0), active(2, 1, 10.8, 0.0)],
            ))
            .unwrap_err();
        assert!(matches!(
            err,
            ArbiterSinkError::PartialSubmit {
                index: 0,
                accepted_prefix: 0,
                decision_len: 1,
                ..
            }
        ));
        assert!(adapter.arbiter().is_scroll_open());
        assert!(adapter.sink().scroll_open);
        assert!(adapter.is_faulted());
        // Cleanup retries the ScrollEnd exactly once.
        adapter.release_all().unwrap();
        let (arbiter, sink) = adapter.into_parts();
        assert_eq!(arbiter.lifecycle(), Lifecycle::Idle);
        assert_eq!(
            sink.events,
            vec![
                OutputEvent::ScrollBegin,
                OutputEvent::ScrollDelta {
                    dx: px(8.0),
                    dy: px(0.0)
                },
                OutputEvent::ScrollEnd,
            ]
        );
        assert!(!sink.scroll_open);
    }

    #[test]
    fn sink_rejected_right_down_owes_no_up() {
        let mut adapter = ArbiterSink::new(two_arbiter_cfg(), ScriptedSink::new(vec![0]));
        adapter
            .frame(&f(0, 0, vec![began(1, 0, 0.0, 0.0)]))
            .unwrap();
        adapter
            .frame(&f(
                1,
                1,
                vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)],
            ))
            .unwrap();
        // Secondary tap decision [RightDown, RightUp] at the release frame:
        // the down (sub 0) is rejected -> nothing was delivered, no up is
        // owed.
        let err = adapter
            .frame(&f(
                2,
                2,
                vec![ended(1, 0, 0.0, 0.0), active(2, 1, 10.0, 0.0)],
            ))
            .unwrap_err();
        assert!(matches!(
            err,
            ArbiterSinkError::PartialSubmit {
                index: 0,
                accepted_prefix: 0,
                decision_len: 2,
                ..
            }
        ));
        assert!(!adapter.arbiter().is_right_held());
        assert!(adapter.is_faulted());
        adapter.release_all().unwrap();
        let (arbiter, sink) = adapter.into_parts();
        assert_eq!(arbiter.lifecycle(), Lifecycle::Idle);
        assert_eq!(sink.events, Vec::<OutputEvent>::new()); // no unmatched up
        assert!(!sink.held_right);
    }

    #[test]
    fn sink_rejected_right_up_after_accepted_down_retries() {
        // Secondary tap [RightDown, RightUp] at the release frame: down
        // (sub 0) accepted, up (sub 1) rejected -> the right stays held and
        // cleanup retries.
        let mut adapter = ArbiterSink::new(two_arbiter_cfg(), ScriptedSink::new(vec![1]));
        adapter
            .frame(&f(0, 0, vec![began(1, 0, 0.0, 0.0)]))
            .unwrap();
        adapter
            .frame(&f(
                1,
                1,
                vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)],
            ))
            .unwrap();
        let err = adapter
            .frame(&f(
                2,
                2,
                vec![ended(1, 0, 0.0, 0.0), active(2, 1, 10.0, 0.0)],
            ))
            .unwrap_err();
        assert!(matches!(
            err,
            ArbiterSinkError::PartialSubmit {
                index: 1,
                accepted_prefix: 1,
                decision_len: 2,
                ..
            }
        ));
        assert!(adapter.arbiter().is_right_held());
        assert!(adapter.sink().held_right);
        assert!(adapter.is_faulted());
        adapter.release_all().unwrap();
        let (arbiter, sink) = adapter.into_parts();
        assert_eq!(arbiter.lifecycle(), Lifecycle::Idle);
        assert_eq!(sink.events, vec![right_down(), right_up()]);
        assert!(!sink.held_right);
    }

    /// R5: with Left and Right held simultaneously, `release_all` reports
    /// **every** failed explicit release structurally (`primary` + `others`),
    /// preserves the exact retry state, and the retry submits exactly the
    /// still-owed releases once.
    ///
    /// The dual-owed state (accepted `ButtonDown(Left)` and
    /// `ButtonDown(Right)` both still held) is reached by a legitimate,
    /// reachable sequence: holding the physical Left and physical Right
    /// buttons at the same time produces two independent held button
    /// sources. It deliberately does **not** rely on the now-invalid
    /// held-button-plus-open-scroll state (review M9 R7): physical button
    /// ownership excludes scroll ownership, so Right held together with an
    /// open scroll is unreachable; scroll cleanup/retry coverage is retained
    /// in separate tests (rejected scroll begin / rejected end / accepted
    /// begin with rejected release).
    #[test]
    fn sink_cleanup_failure_with_left_and_right_held_retries_exact_logs() {
        // Submissions: 0 = LeftDown, 1 = PointerMove (left press + commit
        // frame), 2 = RightDown (second physical press while Left is held),
        // 3 = PointerMove rejected (continuation frame), then on cleanup:
        // 4 = LeftUp rejected, 5 = RightUp rejected, wrapped cleanup fails
        // once; on retry: 6 = LeftUp, 7 = RightUp accepted, wrapped cleanup
        // succeeds.
        let mut adapter = ArbiterSink::new(
            two_arbiter_cfg(),
            ScriptedSink::new(vec![3, 4, 5]).with_release_failures(1),
        );
        adapter
            .frame(&f(0, 0, vec![began(1, 0, 0.0, 0.0)]))
            .unwrap();
        // Physical left press with one finger while the pointer commits:
        // [LeftDown, PointerMove(20, 0)].
        adapter
            .frame(&frm_r(1, 1, vec![active(1, 0, 2.0, 0.0)], true, false))
            .unwrap();
        assert!(adapter.arbiter().is_left_held());
        // Physical right press while Left is still held: [RightDown].
        adapter
            .frame(&frm_r(2, 2, vec![active(1, 0, 2.0, 0.0)], true, true))
            .unwrap();
        assert!(adapter.arbiter().is_left_held());
        assert!(adapter.arbiter().is_right_held());
        // Continued motion while both are held: [PointerMove] rejected ->
        // both held buttons stay delivered/owed.
        let err = adapter
            .frame(&frm_r(3, 3, vec![active(1, 0, 2.5, 0.0)], true, true))
            .unwrap_err();
        assert!(matches!(
            err,
            ArbiterSinkError::PartialSubmit {
                index: 0,
                accepted_prefix: 0,
                decision_len: 1,
                ..
            }
        ));
        assert!(adapter.arbiter().is_left_held());
        assert!(adapter.arbiter().is_right_held());
        assert!(adapter.is_faulted());
        // First cleanup: explicit LeftUp AND RightUp both fail, and the
        // wrapped cleanup fails: the structured error reports BOTH explicit
        // failures (review M9 R5), and both owed releases stay retryable.
        let err = adapter.release_all().unwrap_err();
        match err {
            ArbiterSinkError::ReleaseFailed {
                primary,
                others,
                cleanup,
            } => {
                assert_eq!(
                    primary,
                    Some(OutputError::Rejected(OutputEvent::ButtonUp(
                        MouseButton::Left
                    )))
                );
                assert_eq!(
                    others,
                    vec![OutputError::Rejected(OutputEvent::ButtonUp(
                        MouseButton::Right
                    ))]
                );
                assert!(cleanup.is_some());
            }
            other => panic!("expected ReleaseFailed, got {other:?}"),
        }
        assert!(adapter.arbiter().is_left_held());
        assert!(adapter.arbiter().is_right_held());
        assert!(adapter.is_faulted());
        // Retry: both explicit releases accepted exactly once, wrapped cleanup
        // succeeds.
        adapter.release_all().unwrap();
        let (arbiter, sink) = adapter.into_parts();
        assert_eq!(arbiter.lifecycle(), Lifecycle::Idle);
        assert!(!arbiter.is_left_held());
        assert!(!arbiter.is_right_held());
        assert!(!arbiter.is_scroll_open());
        assert_eq!(
            sink.events,
            vec![
                down(),
                move_event(20.0, 0.0),
                right_down(),
                up(),
                right_up(),
            ]
        );
        assert_eq!(sink.submits, 8);
        assert_eq!(sink.releases, 2);
        assert!(!sink.held_left);
        assert!(!sink.held_right);
        assert!(!sink.scroll_open);
    }

    // ------------------------------------------------------------------
    // M9 review R1–R6 regressions (doc/old/reviews/M9_REVIEW.md, binding)
    // ------------------------------------------------------------------

    /// R1: with `scroll_enabled=false`, centroid motion past the scroll
    /// commit threshold must never open or emit a scroll lifecycle, while a
    /// qualifying quick lift still fires the secondary tap (the scroll
    /// capability is disabled, not the whole family).
    #[test]
    fn scroll_disabled_never_commits_but_secondary_tap_still_fires() {
        let cfg_no_scroll = cfg().with_two_finger(
            TwoFingerConfig::new(
                false, // scroll disabled
                true,
                LogicalPixelsPerMm::try_new(10.0).unwrap(),
                mm(0.5),
                true, // secondary tap enabled
                false,
                dur(500),
                mm(2.0),
            )
            .unwrap(),
        );
        let mut a = Arbiter::new(cfg_no_scroll);
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
                // 1.0 mm centroid movement >= 0.5 mm threshold: must NOT
                // commit a scroll.
                f(2, 2, vec![active(1, 0, 1.0, 0.0), active(2, 1, 11.0, 0.0)]),
                // Even more movement: still no scroll.
                f(3, 3, vec![active(1, 0, 2.0, 0.0), active(2, 1, 12.0, 0.0)]),
                // Quick lift (2.0 mm per contact == tap limit equality): the
                // secondary tap still fires.
                f(4, 4, vec![ended(1, 0, 2.0, 0.0), active(2, 1, 12.0, 0.0)]),
            ],
        );
        assert!(
            scroll_events(&d).is_empty(),
            "scroll disabled: no ScrollBegin/ScrollDelta/ScrollEnd may appear"
        );
        assert!(!a.is_scroll_open());
        assert_eq!(d[2].two_finger_phase_after, TwoFingerPhase::Candidate);
        assert_eq!(d[3].two_finger_phase_after, TwoFingerPhase::Candidate);
        assert_eq!(buttons(&d), vec![right_down(), right_up()]);
        assert_eq!(d[4].two_finger_phase_after, TwoFingerPhase::Finished);
    }

    /// R1: with `scroll_enabled=false` and secondary tap disabled, the
    /// buttonpad two-finger physical click still latches to Right — a
    /// disabled scroll capability never activates, and a disabled tap never
    /// fires — while the physical click capability remains independently
    /// usable.
    #[test]
    fn scroll_disabled_tap_disabled_physical_click_still_latches() {
        let cfg_click_only = cfg().with_two_finger(
            TwoFingerConfig::new(
                false,
                true,
                LogicalPixelsPerMm::try_new(10.0).unwrap(),
                mm(0.5),
                false, // secondary tap disabled
                true,  // buttonpad physical click enabled
                dur(500),
                mm(2.0),
            )
            .unwrap(),
        );
        let mut a = Arbiter::new(cfg_click_only);
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
                // Large centroid movement: never a scroll.
                f(2, 2, vec![active(1, 0, 3.0, 0.0), active(2, 1, 13.0, 0.0)]),
                // Physical-left press with exactly two fingers: latched Right.
                frm(
                    3,
                    3,
                    vec![active(1, 0, 3.0, 0.0), active(2, 1, 13.0, 0.0)],
                    true,
                    false,
                ),
                frm(
                    4,
                    4,
                    vec![active(1, 0, 3.0, 0.0), active(2, 1, 13.0, 0.0)],
                    false,
                    false,
                ),
                // Quick lift: no synthetic tap either.
                f(5, 5, vec![ended(1, 0, 3.0, 0.0), active(2, 1, 13.0, 0.0)]),
            ],
        );
        assert!(scroll_events(&d).is_empty());
        assert_eq!(buttons(&d), vec![right_down(), right_up()]);
        // With scroll and secondary tap both disabled, no candidate ever
        // anchors before the press: the only enabled capability is the
        // buttonpad physical click, which is handled at the press edge.
        assert!(d[..3]
            .iter()
            .all(|d| d.two_finger_phase_after == TwoFingerPhase::Idle));
        assert_eq!(
            d[3].two_finger_phase_after,
            TwoFingerPhase::PhysicalSecondaryClickHeld
        );
        assert_eq!(d[4].two_finger_phase_after, TwoFingerPhase::Finished);
    }

    /// R1: a fully-disabled `TwoFingerConfig` (scroll, tap, and physical
    /// click all off) must not make any capability active merely because an
    /// `Option<TwoFingerConfig>` exists: no candidate, no scroll, no tap.
    #[test]
    fn fully_disabled_two_finger_config_is_inert() {
        let cfg_inert = cfg().with_two_finger(
            TwoFingerConfig::new(
                false,
                true,
                LogicalPixelsPerMm::try_new(10.0).unwrap(),
                mm(0.5),
                false,
                false,
                dur(500),
                mm(2.0),
            )
            .unwrap(),
        );
        let mut a = Arbiter::new(cfg_inert);
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
                f(2, 2, vec![active(1, 0, 3.0, 0.0), active(2, 1, 13.0, 0.0)]),
                f(3, 3, vec![ended(1, 0, 3.0, 0.0), active(2, 1, 13.0, 0.0)]),
            ],
        );
        assert!(d.iter().all(|d| d.events.is_empty()));
        assert!(d
            .iter()
            .all(|d| d.two_finger_phase_after == TwoFingerPhase::Idle));
        assert_eq!(a.two_finger_phase(), TwoFingerPhase::Idle);
    }

    /// R2: a primary physical-left press begun with one finger is followed by
    /// the second finger; while Left is still held at the release boundary,
    /// the two-finger candidate must not synthesize a secondary click — the
    /// continuing contact cluster is permanently tap-disqualified by the
    /// physical button ownership.
    #[test]
    fn physical_left_held_at_release_boundary_blocks_secondary_tap() {
        let mut a = two_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                // Physical left press with one finger: a primary-left press.
                frm(1, 1, vec![active(1, 0, 0.0, 0.0)], true, false),
                // The second finger appears while Left is held: the candidate
                // anchors but the cluster is tap-disqualified.
                frm(
                    2,
                    2,
                    vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)],
                    true,
                    false,
                ),
                // Release boundary with physical Left STILL held: no Right tap.
                frm(
                    3,
                    3,
                    vec![ended(1, 0, 0.0, 0.0), active(2, 1, 10.0, 0.0)],
                    true,
                    false,
                ),
                // The remaining old Active contact emits nothing.
                frm(4, 4, vec![active(2, 1, 10.0, 0.0)], true, false),
                // The matching physical release: exactly one Left up.
                frm(5, 5, vec![], false, false),
            ],
        );
        assert_eq!(
            buttons(&d),
            vec![down(), up()],
            "only the primary physical Left press/release; never a Right tap"
        );
        assert!(!a.is_left_held());
        assert!(!a.is_right_held());
    }

    /// R2: the physical-left ownership disqualification is cluster-level — it
    /// survives the physical release while the same contacts stay down, so a
    /// quick lift after the release still cannot fire a secondary tap.
    #[test]
    fn physical_left_ownership_disqualification_survives_release_in_cluster() {
        let mut a = two_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                frm(1, 1, vec![active(1, 0, 0.0, 0.0)], true, false),
                frm(
                    2,
                    2,
                    vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)],
                    true,
                    false,
                ),
                // Physical left released while both fingers are still down.
                frm(
                    3,
                    3,
                    vec![active(1, 0, 0.0, 0.0), active(2, 1, 10.0, 0.0)],
                    false,
                    false,
                ),
                // Quick lift of the same cluster: still no secondary tap.
                f(4, 4, vec![ended(1, 0, 0.0, 0.0), active(2, 1, 10.0, 0.0)]),
                f(5, 5, vec![ended(2, 1, 10.0, 0.0)]),
            ],
        );
        assert_eq!(buttons(&d), vec![down(), up()]);
        // After the cluster fully drains, a genuinely fresh pair taps
        // normally.
        let d = run(
            &mut a,
            &[
                f(6, 6, vec![began(9, 0, 20.0, 20.0)]),
                f(
                    7,
                    7,
                    vec![active(9, 0, 20.0, 20.0), began(8, 1, 30.0, 20.0)],
                ),
                f(
                    8,
                    8,
                    vec![ended(9, 0, 20.0, 20.0), active(8, 1, 30.0, 20.0)],
                ),
            ],
        );
        assert_eq!(buttons(&d), vec![right_down(), right_up()]);
    }

    /// R2: a one-finger pointer interaction that already committed and emitted
    /// `PointerMove`, followed by a second finger and a quick two-finger
    /// release, must not synthesize a secondary tap — one continuous contact
    /// cluster cannot commit pointer and secondary-tap ownership.
    #[test]
    fn committed_pointer_then_quick_two_finger_release_no_secondary_tap() {
        let mut a = two_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                // The one-finger pointer commits and emits PointerMove.
                f(1, 1, vec![active(1, 0, 2.0, 0.0)]),
                // The second finger appears before the original finger lifts:
                // the committed pointer ownership disqualifies the cluster.
                f(2, 2, vec![active(1, 0, 2.0, 0.0), began(2, 1, 10.0, 0.0)]),
                // Quick small lift: no secondary tap.
                f(3, 3, vec![ended(1, 0, 2.0, 0.0), active(2, 1, 10.0, 0.0)]),
                f(4, 4, vec![ended(2, 1, 10.0, 0.0)]),
            ],
        );
        assert_eq!(moves(&d), vec![(20.0, 0.0)]);
        assert!(
            buttons(&d).is_empty(),
            "no secondary tap after pointer commit"
        );
        assert_eq!(d[2].lifecycle_after, Lifecycle::Cancelled);
        assert_eq!(d[2].two_finger_phase_after, TwoFingerPhase::Candidate);
    }

    /// R2: a still-candidate (not yet committed) one-finger interaction has
    /// emitted no ownership, so a quick two-finger release after the second
    /// finger appears still fires the secondary tap normally.
    #[test]
    fn candidate_pointer_before_second_finger_does_not_disqualify_tap() {
        let mut a = two_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                // Below the pointer threshold: no ownership committed.
                f(1, 1, vec![active(1, 0, 0.5, 0.0)]),
                f(2, 2, vec![active(1, 0, 0.5, 0.0), began(2, 1, 10.0, 0.0)]),
                f(3, 3, vec![ended(1, 0, 0.5, 0.0), active(2, 1, 10.0, 0.0)]),
            ],
        );
        assert_eq!(buttons(&d), vec![right_down(), right_up()]);
    }

    /// R3: a third finger deterministically cancels the candidate; when the
    /// third finger lifts and the original two Active contacts stabilize, the
    /// re-anchored candidate is still tap-disqualified until the cluster
    /// drains — and only a genuinely fresh pair afterwards taps normally.
    #[test]
    fn third_finger_cancel_then_stable_pair_no_tap_fresh_cluster_taps() {
        let mut a = two_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
                // A third finger appears: deterministic cancellation.
                f(
                    2,
                    2,
                    vec![
                        active(1, 0, 0.2, 0.0),
                        active(2, 1, 10.2, 0.0),
                        began(3, 2, 20.0, 20.0),
                    ],
                ),
                // The third finger lifts: back to the original two Active
                // contacts; a fresh candidate re-anchors (tap-disqualified).
                f(3, 3, vec![active(1, 0, 0.2, 0.0), active(2, 1, 10.2, 0.0)]),
                f(4, 4, vec![active(1, 0, 0.2, 0.0), active(2, 1, 10.2, 0.0)]),
                // Quick lift: no secondary tap.
                f(5, 5, vec![ended(1, 0, 0.2, 0.0), active(2, 1, 10.2, 0.0)]),
                f(6, 6, vec![ended(2, 1, 10.2, 0.0)]),
            ],
        );
        assert!(buttons(&d).is_empty());
        assert_eq!(d[2].two_finger_phase_after, TwoFingerPhase::Cancelled);
        assert_eq!(d[3].two_finger_phase_after, TwoFingerPhase::Candidate);
        // After all contacts end (cluster drain), a genuinely fresh pair taps
        // normally.
        let d = run(
            &mut a,
            &[
                f(7, 7, vec![began(4, 0, 30.0, 30.0)]),
                f(
                    8,
                    8,
                    vec![active(4, 0, 30.0, 30.0), began(5, 1, 40.0, 30.0)],
                ),
                f(
                    9,
                    9,
                    vec![ended(4, 0, 30.0, 30.0), active(5, 1, 40.0, 30.0)],
                ),
            ],
        );
        assert_eq!(buttons(&d), vec![right_down(), right_up()]);
    }

    /// R3: missing required coordinates deterministically cancel the
    /// candidate; a later frame with both contacts valid again re-anchors a
    /// tap-disqualified candidate that must not fire a secondary tap.
    #[test]
    fn missing_coordinates_cancel_then_recovery_no_tap() {
        let mut a = two_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
                // Contact 1 loses its required coordinates: cancellation.
                f(
                    2,
                    2,
                    vec![
                        Contact::new(1, 0, ContactState::Active),
                        active(2, 1, 10.0, 0.0),
                    ],
                ),
                // Valid Active recovery: re-anchor (tap-disqualified).
                f(3, 3, vec![active(1, 0, 0.0, 0.0), active(2, 1, 10.0, 0.0)]),
                // Quick lift: no secondary tap.
                f(4, 4, vec![ended(1, 0, 0.0, 0.0), active(2, 1, 10.0, 0.0)]),
            ],
        );
        assert!(buttons(&d).is_empty());
        assert_eq!(d[2].two_finger_phase_after, TwoFingerPhase::Cancelled);
        assert_eq!(d[3].two_finger_phase_after, TwoFingerPhase::Candidate);
    }

    /// R3: a tracking-id replacement cancels the interaction; a later stable
    /// pair re-anchors a tap-disqualified candidate that must not fire a
    /// secondary tap.
    #[test]
    fn tracking_replacement_then_stable_pair_no_tap() {
        let mut a = two_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
                // Tracking-id replacement: 2 -> 12 on slot 1.
                f(2, 2, vec![active(1, 0, 0.2, 0.0), began(12, 1, 10.2, 0.0)]),
                // Stable new pair: re-anchor (tap-disqualified).
                f(3, 3, vec![active(1, 0, 0.2, 0.0), active(12, 1, 10.2, 0.0)]),
                // Quick lift: no secondary tap.
                f(4, 4, vec![ended(1, 0, 0.2, 0.0), active(12, 1, 10.2, 0.0)]),
            ],
        );
        assert!(buttons(&d).is_empty());
        assert_eq!(d[2].two_finger_phase_after, TwoFingerPhase::Cancelled);
        assert_eq!(d[3].two_finger_phase_after, TwoFingerPhase::Candidate);
    }

    /// R3: a sequence regression cancels the interaction fail-closed; a later
    /// monotonic frame re-anchors a tap-disqualified candidate that must not
    /// fire a secondary tap.
    #[test]
    fn regression_cancel_then_later_monotonic_frame_no_tap() {
        let mut a = two_arbiter();
        run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
                f(2, 2, vec![active(1, 0, 0.2, 0.0), active(2, 1, 10.2, 0.0)]),
            ],
        );
        // Sequence regression: rejected, interaction cancelled fail-closed.
        let err = a.frame(&f(2, 3, vec![])).unwrap_err();
        assert!(matches!(err, ArbiterError::SequenceRegression { .. }));
        assert_eq!(a.two_finger_phase(), TwoFingerPhase::Cancelled);
        // A later monotonic frame with both contacts Active re-anchors a
        // tap-disqualified candidate.
        let d = run(
            &mut a,
            &[
                f(3, 4, vec![active(1, 0, 0.2, 0.0), active(2, 1, 10.2, 0.0)]),
                // Quick lift: no secondary tap.
                f(4, 5, vec![ended(1, 0, 0.2, 0.0), active(2, 1, 10.2, 0.0)]),
            ],
        );
        assert!(buttons(&d).is_empty());
        assert_eq!(d[0].two_finger_phase_after, TwoFingerPhase::Candidate);
    }

    /// R4: a physical Right press while a committed scroll is open emits
    /// `ScrollEnd` **before** the new `ButtonDown(Right)` — the old scroll
    /// lifecycle closes before the new click is held (exact same-frame
    /// ordering; runs identically in debug and release profiles).
    #[test]
    fn physical_right_press_while_scrolling_orders_scroll_end_before_down() {
        let (_, d) = two_finger_scroll_run(&[
            f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
            f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
            f(2, 2, vec![active(1, 0, 0.8, 0.0), active(2, 1, 10.8, 0.0)]),
            // Physical right press while scrolling: [ScrollEnd, RightDown].
            frm_r(
                3,
                3,
                vec![active(1, 0, 0.9, 0.0), active(2, 1, 10.9, 0.0)],
                false,
                true,
            ),
        ]);
        assert_eq!(d[3].events, vec![OutputEvent::ScrollEnd, right_down()]);
        assert!(!d[3]
            .events
            .windows(2)
            .any(|w| w[0] == right_down() && w[1] == OutputEvent::ScrollEnd));
        // While the physical press is held the press frame must NOT re-anchor
        // a candidate: physical button ownership excludes scroll ownership,
        // so the cancelled interaction stays cancelled until the button is
        // cleanly released (review M9 R7). The continuing cluster remains
        // tap-disqualified by the press (review M9 R2/R3).
        assert_eq!(d[3].two_finger_phase_after, TwoFingerPhase::Cancelled);
        assert!(!d[3].diagnostics.is_empty());
    }

    /// R4: when a two-finger physical click arrives on the same frame that
    /// establishes the pair while a sticky synthetic Left drag lock is held,
    /// the old synthetic Left up is emitted **before** the newly latched
    /// Right down — never a transient Left+Right chord (exact same-frame
    /// ordering).
    #[test]
    fn two_finger_physical_click_releases_drag_lock_before_right_down() {
        let cfg_combined = cfg().with_tap(tap_cfg()).with_two_finger(two_cfg());
        let mut a = Arbiter::new(cfg_combined);
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![ended(1, 0, 0.1, 0.0)]), // first tap [down, up]
                f(2, 2, vec![began(2, 0, 10.0, 10.0)]), // synthetic [down]
                f(3, 3, vec![active(2, 0, 11.0, 10.0)]), // commit
                f(4, 4, vec![ended(2, 0, 11.0, 10.0)]), // locked: no up
                // Two fingers + physical-left press in the same frame: the
                // latch owns Right, and the drag lock releases first.
                frm(
                    5,
                    5,
                    vec![began(9, 0, 20.0, 20.0), began(8, 1, 30.0, 20.0)],
                    true,
                    false,
                ),
            ],
        );
        assert_eq!(d[5].events, vec![up(), right_down()]);
        assert!(!a.is_left_held());
        assert!(a.is_right_held());
        assert_eq!(
            d[5].two_finger_phase_after,
            TwoFingerPhase::PhysicalSecondaryClickHeld
        );
    }

    /// R6: dropping below two fingers is a `TwoEnd::Release` (potentially a
    /// secondary tap) only when at least one anchored pair member carries a
    /// clean, complete `Ended` record at the first below-two boundary. A
    /// member that simply disappears from the frame cancels without a click.
    #[test]
    fn disappearance_without_ended_record_cancels_without_click() {
        let mut a = two_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
                // Contact 1 vanishes with no Ended record: the candidate is
                // cancelled, never a click.
                f(2, 2, vec![active(2, 1, 10.0, 0.0)]),
                f(3, 3, vec![ended(2, 1, 10.0, 0.0)]),
            ],
        );
        assert!(buttons(&d).is_empty());
        assert_eq!(d[2].two_finger_phase_after, TwoFingerPhase::Cancelled);
    }

    /// R6: release evidence from **at least one** anchored pair member is
    /// sufficient — if one member ends cleanly while the other disappears,
    /// the qualifying secondary tap still fires (its final coordinates count
    /// toward displacement).
    #[test]
    fn one_clean_ended_pair_member_still_qualifies_tap() {
        let mut a = two_arbiter();
        let d = run(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
                // Contact 1 disappears; contact 2 (a pair member) ends
                // cleanly with complete coordinates: a qualifying tap fires.
                f(2, 2, vec![ended(2, 1, 10.0, 0.0)]),
            ],
        );
        assert_eq!(buttons(&d), vec![right_down(), right_up()]);
        assert_eq!(d[2].two_finger_phase_after, TwoFingerPhase::Finished);
    }

    // ------------------------------------------------------------------
    // M9 review R7 regressions (doc/old/reviews/M9_REVIEW.md re-review 1, binding)
    // ------------------------------------------------------------------

    /// Feeds frames one at a time, asserting **after every frame** that no
    /// frame exposes simultaneous physical-button ownership and scroll
    /// ownership (review M9 R7): while aggregate physical Left or Right (or a
    /// latched physical-left-as-right press) is held, `is_scroll_open()` must
    /// be false. Returns all decisions.
    fn run_r7(arbiter: &mut Arbiter, frames: &[ContactFrame]) -> Vec<FrameDecision> {
        frames
            .iter()
            .map(|frame| {
                let decision = arbiter.frame(frame).expect("frame must be accepted");
                let button_held = arbiter.is_physical_left_held()
                    || arbiter.is_physical_right_held()
                    || arbiter.is_latched_right_held();
                assert!(
                    !(button_held && arbiter.is_scroll_open()),
                    "frame {} exposes simultaneous physical-button and scroll ownership",
                    frame.sequence
                );
                decision
            })
            .collect()
    }

    /// R7: a physical Right press begun **before the two-finger pair forms**
    /// excludes scroll ownership while the button is held — no candidate
    /// anchors and no `ScrollBegin`/`ScrollDelta` is emitted during continued
    /// motion — and after the button is cleanly released the same still-live
    /// pair may establish a fresh relative scroll anchor.
    #[test]
    fn physical_right_held_before_pair_blocks_scroll_until_release() {
        let mut a = two_arbiter();
        let d = run_r7(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                // Physical right press with one finger: [RightDown].
                frm_r(1, 1, vec![active(1, 0, 0.0, 0.0)], false, true),
                // The second finger appears while Right is held: no candidate
                // anchors (physical button ownership excludes scroll).
                frm_r(
                    2,
                    2,
                    vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)],
                    false,
                    true,
                ),
                // Continued motion well past the scroll threshold while held:
                // still no scroll lifecycle, phase stays idle.
                frm_r(
                    3,
                    3,
                    vec![active(1, 0, 2.0, 0.0), active(2, 1, 12.0, 0.0)],
                    false,
                    true,
                ),
                frm_r(
                    4,
                    4,
                    vec![active(1, 0, 3.0, 0.0), active(2, 1, 13.0, 0.0)],
                    false,
                    true,
                ),
                // Clean release: [RightUp]; the same still-live pair may
                // re-anchor a fresh relative-scroll candidate.
                frm_r(
                    5,
                    5,
                    vec![active(1, 0, 3.0, 0.0), active(2, 1, 13.0, 0.0)],
                    false,
                    false,
                ),
                // Fresh relative scroll from the post-release anchor works.
                frm_r(
                    6,
                    6,
                    vec![active(1, 0, 3.5, 0.0), active(2, 1, 13.5, 0.0)],
                    false,
                    false,
                ),
            ],
        );
        assert_eq!(d[1].events, vec![right_down()]);
        assert_eq!(d[2].two_finger_phase_after, TwoFingerPhase::Idle);
        assert_eq!(d[3].two_finger_phase_after, TwoFingerPhase::Idle);
        assert_eq!(d[4].two_finger_phase_after, TwoFingerPhase::Idle);
        assert!(scroll_events(&d[..5]).is_empty());
        assert_eq!(d[5].two_finger_phase_after, TwoFingerPhase::Candidate);
        assert_eq!(d[5].events, vec![right_up()]);
        assert_eq!(
            scroll_events(&d[5..]),
            vec![
                OutputEvent::ScrollBegin,
                OutputEvent::ScrollDelta {
                    dx: px(5.0),
                    dy: px(0.0)
                },
            ]
        );
        assert_eq!(d[6].two_finger_phase_after, TwoFingerPhase::CommittedScroll);
        assert!(a.is_scroll_open());
        assert!(!a.is_right_held());
    }

    /// R7: a physical Left press begun **before the two-finger pair forms**
    /// (a primary-left press, not latched — only one finger is present)
    /// excludes scroll ownership while held; after the clean release the same
    /// still-live pair may re-anchor and scroll. Secondary tap stays
    /// cluster-disqualified (the physical press), so a quick lift after the
    /// release still fires nothing.
    #[test]
    fn physical_left_held_before_pair_blocks_scroll_until_release() {
        let mut a = two_arbiter();
        let d = run_r7(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                // Physical left press with one finger: [LeftDown].
                frm_r(1, 1, vec![active(1, 0, 0.0, 0.0)], true, false),
                // The second finger appears while Left is held: no candidate.
                frm_r(
                    2,
                    2,
                    vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)],
                    true,
                    false,
                ),
                // Continued motion while held: no scroll lifecycle.
                frm_r(
                    3,
                    3,
                    vec![active(1, 0, 2.0, 0.0), active(2, 1, 12.0, 0.0)],
                    true,
                    false,
                ),
                // Clean release: [LeftUp]; the pair re-anchors.
                frm_r(
                    4,
                    4,
                    vec![active(1, 0, 2.0, 0.0), active(2, 1, 12.0, 0.0)],
                    false,
                    false,
                ),
                // Fresh relative scroll from the post-release anchor works.
                frm_r(
                    5,
                    5,
                    vec![active(1, 0, 2.5, 0.0), active(2, 1, 12.5, 0.0)],
                    false,
                    false,
                ),
                // Quick lift of the same cluster: still no secondary tap.
                f(6, 6, vec![ended(1, 0, 2.5, 0.0), active(2, 1, 12.5, 0.0)]),
            ],
        );
        assert_eq!(d[1].events, vec![down()]);
        assert_eq!(d[2].two_finger_phase_after, TwoFingerPhase::Idle);
        assert!(scroll_events(&d[..3]).is_empty());
        assert_eq!(d[4].two_finger_phase_after, TwoFingerPhase::Candidate);
        assert_eq!(d[4].events, vec![up()]);
        assert_eq!(
            scroll_events(&d[4..]),
            vec![
                OutputEvent::ScrollBegin,
                OutputEvent::ScrollDelta {
                    dx: px(5.0),
                    dy: px(0.0)
                },
                OutputEvent::ScrollEnd,
            ]
        );
        assert_eq!(d[5].two_finger_phase_after, TwoFingerPhase::CommittedScroll);
        assert!(!buttons(&d)
            .windows(2)
            .any(|w| { w[0] == right_down() && w[1] == right_up() }));
        assert!(!a.is_scroll_open());
        assert!(!a.is_left_held());
    }

    /// R7: a physical Right press **during a committed scroll** emits
    /// `[ScrollEnd, RightDown]` in the same frame (review M9 R4 order) and —
    /// unlike before this repair — the press frame does **not** re-anchor a
    /// candidate: continued motion while the button is held emits no scroll
    /// lifecycle, and after the clean release the same still-live pair
    /// re-anchors and scrolls from a fresh relative anchor.
    #[test]
    fn physical_right_press_during_scroll_blocks_reopen_until_release() {
        let mut a = two_arbiter();
        let d = run_r7(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
                // 0.8 mm centroid movement >= 0.5 mm threshold: commit.
                f(2, 2, vec![active(1, 0, 0.8, 0.0), active(2, 1, 10.8, 0.0)]),
                // Physical right press while scrolling: [ScrollEnd, RightDown];
                // phase Cancelled, no re-anchor while held.
                frm_r(
                    3,
                    3,
                    vec![active(1, 0, 0.9, 0.0), active(2, 1, 10.9, 0.0)],
                    false,
                    true,
                ),
                // Continued motion while held: no scroll re-opens.
                frm_r(
                    4,
                    4,
                    vec![active(1, 0, 1.5, 0.0), active(2, 1, 11.5, 0.0)],
                    false,
                    true,
                ),
                frm_r(
                    5,
                    5,
                    vec![active(1, 0, 2.5, 0.0), active(2, 1, 12.5, 0.0)],
                    false,
                    true,
                ),
                // Clean release: [RightUp]; the pair re-anchors.
                frm_r(
                    6,
                    6,
                    vec![active(1, 0, 2.5, 0.0), active(2, 1, 12.5, 0.0)],
                    false,
                    false,
                ),
                // Fresh relative scroll from the post-release anchor works.
                frm_r(
                    7,
                    7,
                    vec![active(1, 0, 3.0, 0.0), active(2, 1, 13.0, 0.0)],
                    false,
                    false,
                ),
            ],
        );
        assert_eq!(d[3].events, vec![OutputEvent::ScrollEnd, right_down()]);
        assert_eq!(d[3].two_finger_phase_after, TwoFingerPhase::Cancelled);
        assert_eq!(d[4].two_finger_phase_after, TwoFingerPhase::Cancelled);
        assert_eq!(d[5].two_finger_phase_after, TwoFingerPhase::Cancelled);
        assert!(scroll_events(&d[4..6]).is_empty());
        assert_eq!(d[6].two_finger_phase_after, TwoFingerPhase::Candidate);
        assert_eq!(d[6].events, vec![right_up()]);
        assert_eq!(
            scroll_events(&d),
            vec![
                OutputEvent::ScrollBegin,
                OutputEvent::ScrollDelta {
                    dx: px(8.0),
                    dy: px(0.0)
                },
                OutputEvent::ScrollEnd,
                OutputEvent::ScrollBegin,
                OutputEvent::ScrollDelta {
                    dx: px(5.0),
                    dy: px(0.0)
                },
            ]
        );
        assert_eq!(d[7].two_finger_phase_after, TwoFingerPhase::CommittedScroll);
    }

    /// R7: a physical Left press **during a committed scroll** with the
    /// buttonpad physical-click policy disabled is a normal left press (not a
    /// latch) and therefore the same non-latched exclusion applies: same-frame
    /// `[ScrollEnd, LeftDown]`, no re-anchor while held, and after the clean
    /// release the pair re-anchors and scrolls from a fresh relative anchor.
    #[test]
    fn physical_left_press_during_scroll_blocks_reopen_until_release() {
        let cfg_no_click = cfg().with_two_finger(
            TwoFingerConfig::new(
                true,
                true,
                LogicalPixelsPerMm::try_new(10.0).unwrap(),
                mm(0.5),
                true,
                false, // buttonpad two-finger physical click disabled
                dur(500),
                mm(2.0),
            )
            .unwrap(),
        );
        let mut a = Arbiter::new(cfg_no_click);
        let d = run_r7(
            &mut a,
            &[
                f(0, 0, vec![began(1, 0, 0.0, 0.0)]),
                f(1, 1, vec![active(1, 0, 0.0, 0.0), began(2, 1, 10.0, 0.0)]),
                // 0.8 mm centroid movement >= 0.5 mm threshold: commit.
                f(2, 2, vec![active(1, 0, 0.8, 0.0), active(2, 1, 10.8, 0.0)]),
                // Physical left press while scrolling (policy disabled -> a
                // normal left press): [ScrollEnd, LeftDown]; no re-anchor.
                frm_r(
                    3,
                    3,
                    vec![active(1, 0, 0.9, 0.0), active(2, 1, 10.9, 0.0)],
                    true,
                    false,
                ),
                // Continued motion while held: no scroll re-opens.
                frm_r(
                    4,
                    4,
                    vec![active(1, 0, 1.5, 0.0), active(2, 1, 11.5, 0.0)],
                    true,
                    false,
                ),
                frm_r(
                    5,
                    5,
                    vec![active(1, 0, 2.5, 0.0), active(2, 1, 12.5, 0.0)],
                    true,
                    false,
                ),
                // Clean release: [LeftUp]; the pair re-anchors.
                frm_r(
                    6,
                    6,
                    vec![active(1, 0, 2.5, 0.0), active(2, 1, 12.5, 0.0)],
                    false,
                    false,
                ),
                // Fresh relative scroll from the post-release anchor works.
                frm_r(
                    7,
                    7,
                    vec![active(1, 0, 3.0, 0.0), active(2, 1, 13.0, 0.0)],
                    false,
                    false,
                ),
            ],
        );
        assert_eq!(d[3].events, vec![OutputEvent::ScrollEnd, down()]);
        assert_eq!(d[3].two_finger_phase_after, TwoFingerPhase::Cancelled);
        assert_eq!(d[4].two_finger_phase_after, TwoFingerPhase::Cancelled);
        assert_eq!(d[5].two_finger_phase_after, TwoFingerPhase::Cancelled);
        assert!(scroll_events(&d[4..6]).is_empty());
        assert_eq!(d[6].two_finger_phase_after, TwoFingerPhase::Candidate);
        assert_eq!(d[6].events, vec![up()]);
        assert_eq!(
            scroll_events(&d),
            vec![
                OutputEvent::ScrollBegin,
                OutputEvent::ScrollDelta {
                    dx: px(8.0),
                    dy: px(0.0)
                },
                OutputEvent::ScrollEnd,
                OutputEvent::ScrollBegin,
                OutputEvent::ScrollDelta {
                    dx: px(5.0),
                    dy: px(0.0)
                },
            ]
        );
        assert_eq!(d[7].two_finger_phase_after, TwoFingerPhase::CommittedScroll);
        // The post-release scroll is legitimately open — but the physical
        // button was already released, so no frame ever exposed simultaneous
        // physical-button and scroll ownership (run_r7 asserted that).
        assert!(a.is_scroll_open());
        assert!(!a.is_left_held());
    }
}
