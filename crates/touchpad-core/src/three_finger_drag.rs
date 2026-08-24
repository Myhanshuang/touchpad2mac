//! M15 three-finger drag / drag-lock policy.

#![forbid(unsafe_code)]

use std::time::Duration;

use crate::{GestureContact, Monotonic};

#[derive(Clone, Debug, PartialEq)]
pub struct ThreeFingerDragConfig {
    commit_threshold_mm: f64,
    tap_max_displacement_mm: f64,
    tap_max_duration: Duration,
    drag_lock: bool,
    drag_enabled: bool,
    stable_reference_motion: bool,
    /// M19-only entrance stabilization window. Zero preserves the historical
    /// M15-M18 immediate three-finger candidate behavior.
    entry_debounce: Duration,
}

impl ThreeFingerDragConfig {
    pub fn new(
        commit_threshold_mm: f64,
        tap_max_displacement_mm: f64,
        tap_max_duration: Duration,
        drag_lock: bool,
    ) -> Result<Self, ThreeFingerDragConfigError> {
        for (name, value) in [
            ("commit_threshold_mm", commit_threshold_mm),
            ("tap_max_displacement_mm", tap_max_displacement_mm),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(ThreeFingerDragConfigError::NonPositiveOrNonFinite { name, value });
            }
        }
        if tap_max_duration.is_zero() {
            return Err(ThreeFingerDragConfigError::ZeroTapDuration);
        }
        if tap_max_displacement_mm >= commit_threshold_mm {
            return Err(ThreeFingerDragConfigError::InvalidOrdering);
        }
        Ok(Self {
            commit_threshold_mm,
            tap_max_displacement_mm,
            tap_max_duration,
            drag_lock,
            drag_enabled: true,
            stable_reference_motion: false,
            entry_debounce: Duration::ZERO,
        })
    }

    /// Enables/disables only the drag commit. Candidate tracking and the
    /// short three-finger tap semantic remain available, allowing M18 to let
    /// three-finger swipes reach M14 without losing three-finger tap.
    #[must_use]
    pub fn with_drag_enabled(mut self, enabled: bool) -> Self {
        self.drag_enabled = enabled;
        self
    }

    /// Enables the M19 live-use drag-motion refinement: classification
    /// displacement is discarded at commit, motion is measured from one
    /// stable tracking-id reference, and a reference replacement re-baselines
    /// without emitting a jump. Earlier profiles leave this disabled.
    #[must_use]
    pub fn with_stable_reference_motion(mut self, enabled: bool) -> Self {
        self.stable_reference_motion = enabled;
        self
    }

    /// Delays the classifier baseline until a newly-entered contact cluster
    /// has existed for the requested interval. During this window three-finger
    /// motion is owned/suppressed but cannot commit; when the window closes the
    /// classifier is re-anchored once, so touchdown/hand-spread motion cannot
    /// become drag displacement. A zero duration disables the refinement.
    #[must_use]
    pub fn with_entry_debounce(mut self, duration: Duration) -> Self {
        self.entry_debounce = duration;
        self
    }

    #[must_use]
    pub const fn commit_threshold_mm(&self) -> f64 {
        self.commit_threshold_mm
    }
    #[must_use]
    pub const fn tap_max_displacement_mm(&self) -> f64 {
        self.tap_max_displacement_mm
    }
    #[must_use]
    pub const fn tap_max_duration(&self) -> Duration {
        self.tap_max_duration
    }
    #[must_use]
    pub const fn drag_lock(&self) -> bool {
        self.drag_lock
    }
    #[must_use]
    pub const fn drag_enabled(&self) -> bool {
        self.drag_enabled
    }
    #[must_use]
    pub const fn stable_reference_motion(&self) -> bool {
        self.stable_reference_motion
    }
    #[must_use]
    pub const fn entry_debounce(&self) -> Duration {
        self.entry_debounce
    }
}

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum ThreeFingerDragConfigError {
    #[error("M15 drag config requires finite positive {name}, got {value}")]
    NonPositiveOrNonFinite { name: &'static str, value: f64 },
    #[error("M15 three-finger tap duration must be non-zero")]
    ZeroTapDuration,
    #[error("tap displacement must be below drag commit threshold")]
    InvalidOrdering,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThreeFingerDragPhase {
    #[default]
    Idle,
    Candidate,
    Dragging,
    Locked,
    LockedContact,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ThreeFingerDragAction {
    None,
    BeginDrag {
        dx_mm: f64,
        dy_mm: f64,
    },
    /// M19 reference-motion commit: establish drag ownership and a fresh
    /// reference baseline without replaying classifier displacement.
    ArmDrag,
    Move {
        dx_mm: f64,
        dy_mm: f64,
    },
    EndDrag,
    Tap,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThreeFingerDragDecision {
    pub action: ThreeFingerDragAction,
    pub blocks_contact_policy: bool,
}

impl Default for ThreeFingerDragDecision {
    fn default() -> Self {
        Self {
            action: ThreeFingerDragAction::None,
            blocks_contact_policy: false,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ThreeFingerDragState {
    phase: ThreeFingerDragPhase,
    ids: Vec<i32>,
    /// Start of the current non-empty touch cluster. M19 uses this rather
    /// than the third-finger timestamp, matching linux-3-finger-drag's entry
    /// debounce: slowly staged fingers have already paid the debounce by the
    /// time the third arrives, while a simultaneous fast swipe has not.
    entry_began: Option<Monotonic>,
    /// Whether the M19 entrance debounce has already resolved. Historical
    /// profiles keep this true immediately. For a fast three-finger entry,
    /// resolving the window arms the drag directly instead of starting a
    /// second movement-classification window.
    entry_debounce_reanchored: bool,
    anchor: Option<(f64, f64)>,
    last: Option<(f64, f64)>,
    reference_id: Option<i32>,
    reference_last: Option<(f64, f64)>,
    /// A direct post-debounce arm does not press the button. Track whether
    /// reference motion happened afterwards so a stationary short three-
    /// finger contact can still retain the existing semantic-tap behavior.
    drag_motion_seen: bool,
    began: Option<Monotonic>,
    max_displacement_mm: f64,
}

impl ThreeFingerDragState {
    #[must_use]
    pub const fn phase(&self) -> ThreeFingerDragPhase {
        self.phase
    }
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

pub fn process_three_finger_drag(
    config: &ThreeFingerDragConfig,
    state: &mut ThreeFingerDragState,
    contacts: &[GestureContact],
    timestamp: Monotonic,
) -> ThreeFingerDragDecision {
    let mut live = contacts.to_vec();
    live.sort_by_key(|contact| contact.tracking_id);
    let ids: Vec<i32> = live.iter().map(|c| c.tracking_id).collect();

    match state.phase {
        ThreeFingerDragPhase::Idle => {
            if !config.entry_debounce.is_zero() {
                if live.is_empty() {
                    state.entry_began = None;
                    state.entry_debounce_reanchored = false;
                    return ThreeFingerDragDecision::default();
                }
                if state.entry_began.is_none() {
                    state.entry_began = Some(timestamp);
                }
            }
            if live.len() == 3 {
                start_candidate(config, state, &live, timestamp, false);
                if config.drag_enabled && !state.entry_debounce_reanchored {
                    // Match the reference project's buffered entry period:
                    // once all three fingers are present, do not let their
                    // touchdown/settling motion leak into lower policies while
                    // the cluster is still inside the debounce window.
                    return ThreeFingerDragDecision {
                        action: ThreeFingerDragAction::None,
                        blocks_contact_policy: true,
                    };
                }
            }
            ThreeFingerDragDecision::default()
        }
        ThreeFingerDragPhase::Candidate => {
            if live.len() != 3 || ids != state.ids {
                let tap = live.is_empty() && qualifies_tap(config, state, timestamp);
                state.reset();
                return ThreeFingerDragDecision {
                    action: if tap {
                        ThreeFingerDragAction::Tap
                    } else {
                        ThreeFingerDragAction::None
                    },
                    blocks_contact_policy: tap,
                };
            }
            update_candidate(config, state, &live, timestamp, false)
        }
        ThreeFingerDragPhase::Dragging => {
            if live.is_empty() {
                if config.stable_reference_motion
                    && !state.drag_motion_seen
                    && qualifies_tap(config, state, timestamp)
                {
                    state.reset();
                    return ThreeFingerDragDecision {
                        action: ThreeFingerDragAction::Tap,
                        blocks_contact_policy: true,
                    };
                }
                if config.drag_lock {
                    state.phase = ThreeFingerDragPhase::Locked;
                    state.ids.clear();
                    state.anchor = None;
                    state.last = None;
                    state.reference_id = None;
                    state.reference_last = None;
                    state.began = None;
                    state.max_displacement_mm = 0.0;
                    return ThreeFingerDragDecision {
                        action: ThreeFingerDragAction::None,
                        blocks_contact_policy: true,
                    };
                }
                state.reset();
                return ThreeFingerDragDecision {
                    action: ThreeFingerDragAction::EndDrag,
                    blocks_contact_policy: true,
                };
            }
            if config.stable_reference_motion {
                if !ids.iter().all(|id| state.ids.contains(id)) {
                    // A replacement/new contact is not a clean staggered lift.
                    // Release fail-closed rather than carrying drag ownership
                    // into an unrelated contact cluster.
                    state.reset();
                    return ThreeFingerDragDecision {
                        action: ThreeFingerDragAction::EndDrag,
                        blocks_contact_policy: true,
                    };
                }
                return drive_reference(state, &live);
            }

            // Historical M15-M18 centroid behavior is preserved verbatim.
            if live.len() == 2 && ids.iter().all(|id| state.ids.contains(id)) {
                return ThreeFingerDragDecision {
                    action: ThreeFingerDragAction::None,
                    blocks_contact_policy: true,
                };
            }
            if live.len() == 1 && ids.iter().all(|id| state.ids.contains(id)) {
                state.reset();
                return ThreeFingerDragDecision {
                    action: ThreeFingerDragAction::EndDrag,
                    blocks_contact_policy: true,
                };
            }
            if live.len() != 3 || ids != state.ids {
                state.reset();
                return ThreeFingerDragDecision {
                    action: ThreeFingerDragAction::EndDrag,
                    blocks_contact_policy: true,
                };
            }
            let current = centroid(&live);
            let last = state.last.unwrap_or(current);
            state.last = Some(current);
            ThreeFingerDragDecision {
                action: ThreeFingerDragAction::Move {
                    dx_mm: current.0 - last.0,
                    dy_mm: current.1 - last.1,
                },
                blocks_contact_policy: true,
            }
        }
        ThreeFingerDragPhase::Locked => {
            if live.len() == 3 {
                start_candidate(config, state, &live, timestamp, true);
                ThreeFingerDragDecision {
                    action: ThreeFingerDragAction::None,
                    blocks_contact_policy: true,
                }
            } else {
                ThreeFingerDragDecision {
                    action: ThreeFingerDragAction::None,
                    blocks_contact_policy: true,
                }
            }
        }
        ThreeFingerDragPhase::LockedContact => {
            if live.len() != 3 || ids != state.ids {
                if live.is_empty() && qualifies_tap(config, state, timestamp) {
                    state.reset();
                    return ThreeFingerDragDecision {
                        action: ThreeFingerDragAction::EndDrag,
                        blocks_contact_policy: true,
                    };
                }
                state.phase = ThreeFingerDragPhase::Locked;
                state.ids.clear();
                state.anchor = None;
                state.last = None;
                state.reference_id = None;
                state.reference_last = None;
                return ThreeFingerDragDecision {
                    action: ThreeFingerDragAction::None,
                    blocks_contact_policy: true,
                };
            }
            update_candidate(config, state, &live, timestamp, true)
        }
    }
}

fn start_candidate(
    config: &ThreeFingerDragConfig,
    state: &mut ThreeFingerDragState,
    contacts: &[GestureContact],
    timestamp: Monotonic,
    locked: bool,
) {
    let center = centroid(contacts);
    state.phase = if locked {
        ThreeFingerDragPhase::LockedContact
    } else {
        ThreeFingerDragPhase::Candidate
    };
    state.ids = contacts.iter().map(|c| c.tracking_id).collect();
    state.anchor = Some(center);
    state.last = Some(center);
    state.entry_debounce_reanchored = locked
        || config.entry_debounce.is_zero()
        || state.entry_began.is_some_and(|began| {
            timestamp
                .duration_since(began)
                .is_some_and(|elapsed| elapsed >= config.entry_debounce)
        });
    state.reference_id = None;
    state.reference_last = None;
    state.drag_motion_seen = false;
    state.began = Some(timestamp);
    state.max_displacement_mm = 0.0;
}

fn update_candidate(
    config: &ThreeFingerDragConfig,
    state: &mut ThreeFingerDragState,
    contacts: &[GestureContact],
    timestamp: Monotonic,
    already_held: bool,
) -> ThreeFingerDragDecision {
    let current = centroid(contacts);
    let anchor = state.anchor.unwrap_or(current);
    let displacement = (current.0 - anchor.0).hypot(current.1 - anchor.1);
    state.max_displacement_mm = state.max_displacement_mm.max(displacement);

    if !already_held && !state.entry_debounce_reanchored {
        let ready = state.entry_began.is_some_and(|began| {
            timestamp
                .duration_since(began)
                .is_some_and(|elapsed| elapsed >= config.entry_debounce)
        });
        if !ready {
            state.last = Some(current);
            return ThreeFingerDragDecision {
                action: ThreeFingerDragAction::None,
                blocks_contact_policy: config.drag_enabled,
            };
        }

        // The cluster has now existed for the full entrance window. The
        // reference implementation resolves a stable three-finger touch here:
        // buffered touchdown motion is classification evidence but is never
        // replayed, and there is no *second* movement threshold after the
        // debounce. Requiring another threshold here delayed fast flicks into
        // their high-speed phase and made the first post-press delta much
        // larger. Arm at the current position and use it as the fresh motion
        // baseline; the synthetic button is still deferred until a real Move.
        state.last = Some(current);
        state.entry_debounce_reanchored = true;
        if config.drag_enabled && config.stable_reference_motion {
            state.phase = ThreeFingerDragPhase::Dragging;
            baseline_reference(state, contacts);
            return ThreeFingerDragDecision {
                action: ThreeFingerDragAction::ArmDrag,
                blocks_contact_policy: true,
            };
        }

        // Defensive fallback for a future non-stable profile that opts into
        // entry debounce, and for a disabled M19 drag: preserve the previous
        // threshold/tap classifier semantics from a fresh post-window anchor.
        state.anchor = Some(current);
        return ThreeFingerDragDecision {
            action: ThreeFingerDragAction::None,
            blocks_contact_policy: config.drag_enabled,
        };
    }

    if !config.drag_enabled {
        state.last = Some(current);
        return ThreeFingerDragDecision {
            action: ThreeFingerDragAction::None,
            blocks_contact_policy: already_held,
        };
    }
    if displacement >= config.commit_threshold_mm {
        let last = state.last.unwrap_or(anchor);
        state.last = Some(current);
        state.phase = ThreeFingerDragPhase::Dragging;
        if config.stable_reference_motion {
            baseline_reference(state, contacts);
            ThreeFingerDragDecision {
                action: ThreeFingerDragAction::ArmDrag,
                blocks_contact_policy: true,
            }
        } else {
            ThreeFingerDragDecision {
                action: if already_held {
                    ThreeFingerDragAction::Move {
                        dx_mm: current.0 - last.0,
                        dy_mm: current.1 - last.1,
                    }
                } else {
                    ThreeFingerDragAction::BeginDrag {
                        dx_mm: current.0 - anchor.0,
                        dy_mm: current.1 - anchor.1,
                    }
                },
                blocks_contact_policy: true,
            }
        }
    } else {
        state.last = Some(current);
        ThreeFingerDragDecision {
            action: ThreeFingerDragAction::None,
            blocks_contact_policy: already_held,
        }
    }
}

/// Chooses a stable tracking-id reference and records its current position.
/// The commit frame intentionally produces no movement: displacement used to
/// classify the gesture must never be replayed after the synthetic press.
fn baseline_reference(state: &mut ThreeFingerDragState, contacts: &[GestureContact]) {
    if let Some(reference) = contacts.first() {
        state.reference_id = Some(reference.tracking_id);
        state.reference_last = Some((reference.x_mm, reference.y_mm));
    } else {
        state.reference_id = None;
        state.reference_last = None;
    }
}

/// Emits drag motion from one stable reference finger. Clean staggered lifts
/// stay owned until the cluster is empty. If the reference finger lifts before
/// the others, another surviving original finger is selected and re-baselined
/// with no delta on that frame, preventing a centroid/finger-count jump.
fn drive_reference(
    state: &mut ThreeFingerDragState,
    contacts: &[GestureContact],
) -> ThreeFingerDragDecision {
    let reference = state
        .reference_id
        .and_then(|id| contacts.iter().find(|contact| contact.tracking_id == id));

    let Some(reference) = reference else {
        baseline_reference(state, contacts);
        return ThreeFingerDragDecision {
            action: ThreeFingerDragAction::None,
            blocks_contact_policy: true,
        };
    };

    let current = (reference.x_mm, reference.y_mm);
    let last = state.reference_last.unwrap_or(current);
    state.reference_last = Some(current);
    if current == last {
        return ThreeFingerDragDecision {
            action: ThreeFingerDragAction::None,
            blocks_contact_policy: true,
        };
    }
    state.drag_motion_seen = true;
    ThreeFingerDragDecision {
        action: ThreeFingerDragAction::Move {
            dx_mm: current.0 - last.0,
            dy_mm: current.1 - last.1,
        },
        blocks_contact_policy: true,
    }
}

fn qualifies_tap(
    config: &ThreeFingerDragConfig,
    state: &ThreeFingerDragState,
    timestamp: Monotonic,
) -> bool {
    state.max_displacement_mm <= config.tap_max_displacement_mm
        && state.began.is_some_and(|began| {
            timestamp
                .duration_since(began)
                .is_some_and(|elapsed| elapsed <= config.tap_max_duration)
        })
}

fn centroid(contacts: &[GestureContact]) -> (f64, f64) {
    let n = contacts.len() as f64;
    (
        contacts.iter().map(|c| c.x_mm).sum::<f64>() / n,
        contacts.iter().map(|c| c.y_mm).sum::<f64>() / n,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(lock: bool) -> ThreeFingerDragConfig {
        ThreeFingerDragConfig::new(1.0, 0.5, Duration::from_millis(200), lock).unwrap()
    }
    fn stable_cfg(lock: bool) -> ThreeFingerDragConfig {
        cfg(lock).with_stable_reference_motion(true)
    }
    fn debounced_stable_cfg(lock: bool) -> ThreeFingerDragConfig {
        stable_cfg(lock).with_entry_debounce(Duration::from_millis(50))
    }
    fn contacts(x: f64) -> Vec<GestureContact> {
        vec![
            GestureContact {
                tracking_id: 1,
                x_mm: x,
                y_mm: 0.0,
                role: None,
            },
            GestureContact {
                tracking_id: 2,
                x_mm: x + 5.0,
                y_mm: 0.0,
                role: None,
            },
            GestureContact {
                tracking_id: 3,
                x_mm: x + 10.0,
                y_mm: 0.0,
                role: None,
            },
        ]
    }

    #[test]
    fn drag_commits_before_m14_swipe_threshold() {
        let mut state = ThreeFingerDragState::default();
        let _ = process_three_finger_drag(&cfg(false), &mut state, &contacts(0.0), Monotonic::ZERO);
        let d = process_three_finger_drag(
            &cfg(false),
            &mut state,
            &contacts(1.2),
            Monotonic::from_nanos(10_000_000),
        );
        assert!(matches!(d.action, ThreeFingerDragAction::BeginDrag { .. }));
    }

    #[test]
    fn commit_rebaselines_instead_of_replaying_classifier_displacement() {
        let mut state = ThreeFingerDragState::default();
        let _ = process_three_finger_drag(
            &stable_cfg(false),
            &mut state,
            &contacts(0.0),
            Monotonic::ZERO,
        );

        // Crossing the 1 mm classifier threshold establishes drag ownership
        // but does not replay the accumulated 1.2 mm under a synthetic press.
        let committed = process_three_finger_drag(
            &stable_cfg(false),
            &mut state,
            &contacts(1.2),
            Monotonic::from_nanos(10_000_000),
        );
        assert_eq!(committed.action, ThreeFingerDragAction::ArmDrag);

        let stationary = process_three_finger_drag(
            &stable_cfg(false),
            &mut state,
            &contacts(1.2),
            Monotonic::from_nanos(16_000_000),
        );
        assert_eq!(stationary.action, ThreeFingerDragAction::None);

        let moved = process_three_finger_drag(
            &stable_cfg(false),
            &mut state,
            &contacts(1.7),
            Monotonic::from_nanos(22_000_000),
        );
        assert_eq!(
            moved.action,
            ThreeFingerDragAction::Move {
                dx_mm: 0.5,
                dy_mm: 0.0
            }
        );
    }

    #[test]
    fn fast_three_finger_entry_discards_touchdown_motion_until_debounce_expires() {
        let config = debounced_stable_cfg(false);
        let mut state = ThreeFingerDragState::default();

        let one = vec![contacts(0.0)[0]];
        let first = process_three_finger_drag(&config, &mut state, &one, Monotonic::ZERO);
        assert_eq!(first.action, ThreeFingerDragAction::None);

        let entered = process_three_finger_drag(
            &config,
            &mut state,
            &contacts(0.2),
            Monotonic::from_nanos(8_000_000),
        );
        assert_eq!(entered.action, ThreeFingerDragAction::None);
        assert!(entered.blocks_contact_policy);

        let early_flick = process_three_finger_drag(
            &config,
            &mut state,
            &contacts(4.0),
            Monotonic::from_nanos(30_000_000),
        );
        assert_eq!(early_flick.action, ThreeFingerDragAction::None);
        assert!(early_flick.blocks_contact_policy);
        assert_eq!(state.phase(), ThreeFingerDragPhase::Candidate);

        let settled = process_three_finger_drag(
            &config,
            &mut state,
            &contacts(4.5),
            Monotonic::from_nanos(50_000_000),
        );
        assert_eq!(settled.action, ThreeFingerDragAction::ArmDrag);
        assert!(settled.blocks_contact_policy);
        assert_eq!(state.phase(), ThreeFingerDragPhase::Dragging);

        let moved = process_three_finger_drag(
            &config,
            &mut state,
            &contacts(5.7),
            Monotonic::from_nanos(60_000_000),
        );
        match moved.action {
            ThreeFingerDragAction::Move { dx_mm, dy_mm } => {
                assert!((dx_mm - 1.2).abs() < 1e-9);
                assert!(dy_mm.abs() < 1e-9);
            }
            other => panic!("expected post-debounce reference motion, got {other:?}"),
        }
    }

    #[test]
    fn debounced_stationary_three_finger_contact_can_still_tap() {
        let config = debounced_stable_cfg(false);
        let mut state = ThreeFingerDragState::default();

        let one = vec![contacts(0.0)[0]];
        let _ = process_three_finger_drag(&config, &mut state, &one, Monotonic::ZERO);
        let _ = process_three_finger_drag(
            &config,
            &mut state,
            &contacts(0.0),
            Monotonic::from_nanos(8_000_000),
        );
        let armed = process_three_finger_drag(
            &config,
            &mut state,
            &contacts(0.0),
            Monotonic::from_nanos(50_000_000),
        );
        assert_eq!(armed.action, ThreeFingerDragAction::ArmDrag);

        let tap =
            process_three_finger_drag(&config, &mut state, &[], Monotonic::from_nanos(100_000_000));
        assert_eq!(tap.action, ThreeFingerDragAction::Tap);
        assert!(tap.blocks_contact_policy);
    }

    #[test]
    fn staged_fingers_pay_entry_debounce_before_third_finger_arrives() {
        let config = debounced_stable_cfg(false);
        let mut state = ThreeFingerDragState::default();

        let all = contacts(0.0);
        let one = vec![all[0]];
        let two = vec![all[0], all[1]];
        let _ = process_three_finger_drag(&config, &mut state, &one, Monotonic::ZERO);
        let _ =
            process_three_finger_drag(&config, &mut state, &two, Monotonic::from_nanos(30_000_000));

        let entered =
            process_three_finger_drag(&config, &mut state, &all, Monotonic::from_nanos(60_000_000));
        assert_eq!(entered.action, ThreeFingerDragAction::None);
        assert!(!entered.blocks_contact_policy);

        let committed = process_three_finger_drag(
            &config,
            &mut state,
            &contacts(1.2),
            Monotonic::from_nanos(70_000_000),
        );
        assert_eq!(committed.action, ThreeFingerDragAction::ArmDrag);
    }

    #[test]
    fn reference_finger_lift_rebaselines_without_position_jump() {
        let mut state = ThreeFingerDragState::default();
        let _ = process_three_finger_drag(
            &stable_cfg(false),
            &mut state,
            &contacts(0.0),
            Monotonic::ZERO,
        );
        let _ = process_three_finger_drag(
            &stable_cfg(false),
            &mut state,
            &contacts(1.2),
            Monotonic::from_nanos(10_000_000),
        );

        let moved = process_three_finger_drag(
            &stable_cfg(false),
            &mut state,
            &contacts(1.6),
            Monotonic::from_nanos(16_000_000),
        );
        match moved.action {
            ThreeFingerDragAction::Move { dx_mm, dy_mm } => {
                assert!((dx_mm - 0.4).abs() < 1e-9);
                assert!(dy_mm.abs() < 1e-9);
            }
            other => panic!("expected reference-finger motion, got {other:?}"),
        }

        // Reference id 1 lifts. The surviving fingers are deliberately far
        // away in absolute coordinates; selecting id 2 must baseline there
        // without turning that absolute gap into cursor movement.
        let mut two = contacts(20.0);
        two.remove(0);
        let switched = process_three_finger_drag(
            &stable_cfg(false),
            &mut state,
            &two,
            Monotonic::from_nanos(22_000_000),
        );
        assert_eq!(switched.action, ThreeFingerDragAction::None);

        let mut two_moved = contacts(20.4);
        two_moved.remove(0);
        let resumed = process_three_finger_drag(
            &stable_cfg(false),
            &mut state,
            &two_moved,
            Monotonic::from_nanos(28_000_000),
        );
        match resumed.action {
            ThreeFingerDragAction::Move { dx_mm, dy_mm } => {
                assert!((dx_mm - 0.4).abs() < 1e-9);
                assert!(dy_mm.abs() < 1e-9);
            }
            other => panic!("expected resumed reference-finger motion, got {other:?}"),
        }
    }

    #[test]
    fn committed_drag_keeps_staggered_tail_until_cluster_is_empty() {
        let mut state = ThreeFingerDragState::default();
        let _ = process_three_finger_drag(
            &stable_cfg(false),
            &mut state,
            &contacts(0.0),
            Monotonic::ZERO,
        );
        let _ = process_three_finger_drag(
            &stable_cfg(false),
            &mut state,
            &contacts(1.2),
            Monotonic::from_nanos(10_000_000),
        );
        assert_eq!(state.phase(), ThreeFingerDragPhase::Dragging);

        // One lifted finger (3 -> 2) keeps drag ownership but emits no motion.
        let mut two = contacts(1.2);
        two.pop();
        let partial = process_three_finger_drag(
            &stable_cfg(false),
            &mut state,
            &two,
            Monotonic::from_nanos(16_000_000),
        );
        assert_eq!(partial.action, ThreeFingerDragAction::None);
        assert!(partial.blocks_contact_policy);
        assert_eq!(state.phase(), ThreeFingerDragPhase::Dragging);

        // Keep suppressing through 3 -> 1 as well. The remaining original
        // finger still belongs to the committed drag until the cluster is
        // fully empty.
        let one = vec![two[0]];
        let one_tail = process_three_finger_drag(
            &stable_cfg(false),
            &mut state,
            &one,
            Monotonic::from_nanos(22_000_000),
        );
        assert_eq!(one_tail.action, ThreeFingerDragAction::None);
        assert!(one_tail.blocks_contact_policy);
        assert_eq!(state.phase(), ThreeFingerDragPhase::Dragging);

        let ended = process_three_finger_drag(
            &stable_cfg(false),
            &mut state,
            &[],
            Monotonic::from_nanos(28_000_000),
        );
        assert_eq!(ended.action, ThreeFingerDragAction::EndDrag);
        assert!(ended.blocks_contact_policy);
        assert_eq!(state.phase(), ThreeFingerDragPhase::Idle);
    }

    #[test]
    fn clean_drag_locks_and_three_finger_tap_unlocks() {
        let mut state = ThreeFingerDragState::default();
        let _ = process_three_finger_drag(&cfg(true), &mut state, &contacts(0.0), Monotonic::ZERO);
        let _ = process_three_finger_drag(
            &cfg(true),
            &mut state,
            &contacts(1.2),
            Monotonic::from_nanos(10_000_000),
        );
        let locked = process_three_finger_drag(
            &cfg(true),
            &mut state,
            &[],
            Monotonic::from_nanos(20_000_000),
        );
        assert_eq!(state.phase(), ThreeFingerDragPhase::Locked);
        assert_eq!(locked.action, ThreeFingerDragAction::None);
        let _ = process_three_finger_drag(
            &cfg(true),
            &mut state,
            &contacts(2.0),
            Monotonic::from_nanos(30_000_000),
        );
        let end = process_three_finger_drag(
            &cfg(true),
            &mut state,
            &[],
            Monotonic::from_nanos(50_000_000),
        );
        assert_eq!(end.action, ThreeFingerDragAction::EndDrag);
    }

    #[test]
    fn short_three_finger_tap_is_semantic_tap() {
        let mut state = ThreeFingerDragState::default();
        let _ = process_three_finger_drag(&cfg(false), &mut state, &contacts(0.0), Monotonic::ZERO);
        let tap = process_three_finger_drag(
            &cfg(false),
            &mut state,
            &[],
            Monotonic::from_nanos(100_000_000),
        );
        assert_eq!(tap.action, ThreeFingerDragAction::Tap);
    }
}
