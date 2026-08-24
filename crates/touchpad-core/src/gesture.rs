//! M14 platform-neutral continuous gesture recognizer.

#![forbid(unsafe_code)]

use crate::{ContactRole, ContinuousGestureEvent, ContinuousGestureKind, ContinuousGesturePhase};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GestureContact {
    pub tracking_id: i32,
    pub x_mm: f64,
    pub y_mm: f64,
    pub role: Option<ContactRole>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GestureConfig {
    pinch_commit_mm: f64,
    rotate_commit_radians: f64,
    page_swipe_commit_mm: f64,
    page_swipe_dominance: f64,
    scroll_translation_win_mm: f64,
    multi_swipe_commit_mm: f64,
    edge_commit_mm: f64,
    edge_zone_mm: f64,
    surface_size_mm: Option<(f64, f64)>,
}

impl GestureConfig {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pinch_commit_mm: f64,
        rotate_commit_radians: f64,
        page_swipe_commit_mm: f64,
        page_swipe_dominance: f64,
        scroll_translation_win_mm: f64,
        multi_swipe_commit_mm: f64,
        edge_commit_mm: f64,
        edge_zone_mm: f64,
    ) -> Result<Self, GestureConfigError> {
        for (name, value) in [
            ("pinch_commit_mm", pinch_commit_mm),
            ("rotate_commit_radians", rotate_commit_radians),
            ("page_swipe_commit_mm", page_swipe_commit_mm),
            ("page_swipe_dominance", page_swipe_dominance),
            ("scroll_translation_win_mm", scroll_translation_win_mm),
            ("multi_swipe_commit_mm", multi_swipe_commit_mm),
            ("edge_commit_mm", edge_commit_mm),
            ("edge_zone_mm", edge_zone_mm),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(GestureConfigError::NonPositiveOrNonFinite { name, value });
            }
        }
        if page_swipe_dominance <= 1.0 {
            return Err(GestureConfigError::InvalidOrdering(
                "page_swipe_dominance must be greater than 1",
            ));
        }
        Ok(Self {
            pinch_commit_mm,
            rotate_commit_radians,
            page_swipe_commit_mm,
            page_swipe_dominance,
            scroll_translation_win_mm,
            multi_swipe_commit_mm,
            edge_commit_mm,
            edge_zone_mm,
            surface_size_mm: None,
        })
    }

    pub fn with_surface_size_mm(
        mut self,
        width: f64,
        height: f64,
    ) -> Result<Self, GestureConfigError> {
        if !width.is_finite()
            || !height.is_finite()
            || width <= 2.0 * self.edge_zone_mm
            || height <= 2.0 * self.edge_zone_mm
        {
            return Err(GestureConfigError::InvalidSurface);
        }
        self.surface_size_mm = Some((width, height));
        Ok(self)
    }

    #[must_use]
    pub const fn pinch_commit_mm(&self) -> f64 {
        self.pinch_commit_mm
    }
    #[must_use]
    pub const fn rotate_commit_radians(&self) -> f64 {
        self.rotate_commit_radians
    }
    #[must_use]
    pub const fn page_swipe_commit_mm(&self) -> f64 {
        self.page_swipe_commit_mm
    }
    #[must_use]
    pub const fn page_swipe_dominance(&self) -> f64 {
        self.page_swipe_dominance
    }
    #[must_use]
    pub const fn scroll_translation_win_mm(&self) -> f64 {
        self.scroll_translation_win_mm
    }
    #[must_use]
    pub const fn multi_swipe_commit_mm(&self) -> f64 {
        self.multi_swipe_commit_mm
    }
    #[must_use]
    pub const fn edge_commit_mm(&self) -> f64 {
        self.edge_commit_mm
    }
    #[must_use]
    pub const fn edge_zone_mm(&self) -> f64 {
        self.edge_zone_mm
    }
    #[must_use]
    pub const fn surface_size_mm(&self) -> Option<(f64, f64)> {
        self.surface_size_mm
    }
}

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum GestureConfigError {
    #[error("M14 gesture config requires finite positive {name}, got {value}")]
    NonPositiveOrNonFinite { name: &'static str, value: f64 },
    #[error("invalid M14 gesture ordering: {0}")]
    InvalidOrdering(&'static str),
    #[error("invalid M14 gesture touch-surface dimensions")]
    InvalidSurface,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GestureState {
    anchor: Vec<GestureContact>,
    committed: Option<ContinuousGestureKind>,
    edge_started: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GestureDecision {
    pub events: Vec<ContinuousGestureEvent>,
    pub blocks_contact_policy: bool,
}

impl GestureState {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    #[must_use]
    pub const fn committed(&self) -> Option<ContinuousGestureKind> {
        self.committed
    }

    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.anchor.is_empty() && self.committed.is_none()
    }
}

pub fn process_gesture(
    config: &GestureConfig,
    state: &mut GestureState,
    contacts: &[GestureContact],
) -> GestureDecision {
    let mut current = contacts.to_vec();
    current.sort_by_key(|contact| contact.tracking_id);

    if let Some(kind) = state.committed {
        if !same_ids(&state.anchor, &current) {
            state.reset();
            return GestureDecision {
                events: vec![event(kind, ContinuousGesturePhase::End, 0.0, 0.0, 1.0, 0.0)],
                blocks_contact_policy: true,
            };
        }
        let m = metrics(&state.anchor, &current);
        return GestureDecision {
            events: vec![event(
                kind,
                ContinuousGesturePhase::Update,
                m.translation.0,
                m.translation.1,
                m.scale,
                m.rotation,
            )],
            blocks_contact_policy: true,
        };
    }

    if !(2..=4).contains(&current.len()) {
        state.reset();
        return GestureDecision::default();
    }
    if state.anchor.is_empty() || !same_ids(&state.anchor, &current) {
        state.anchor = current;
        state.edge_started = edge_started(config, &state.anchor);
        return GestureDecision::default();
    }

    let m = metrics(&state.anchor, &current);
    let translation_norm = m.translation.0.hypot(m.translation.1);
    let kind = match current.len() {
        2 => {
            if m.span_delta.abs() >= config.pinch_commit_mm
                && translation_norm < config.scroll_translation_win_mm
            {
                Some(ContinuousGestureKind::Pinch)
            } else if m.rotation.abs() >= config.rotate_commit_radians
                && translation_norm < config.scroll_translation_win_mm
            {
                Some(ContinuousGestureKind::Rotate)
            } else if m.translation.0.abs() >= config.page_swipe_commit_mm
                && m.translation.0.abs() >= m.translation.1.abs() * config.page_swipe_dominance
                && config.page_swipe_commit_mm < config.scroll_translation_win_mm
            {
                Some(ContinuousGestureKind::TwoFingerPageSwipe)
            } else {
                if translation_norm >= config.scroll_translation_win_mm {
                    state.reset();
                }
                None
            }
        }
        3 => {
            if state.edge_started && translation_norm >= config.edge_commit_mm {
                Some(ContinuousGestureKind::EdgeSwipe)
            } else if translation_norm >= config.multi_swipe_commit_mm {
                Some(ContinuousGestureKind::ThreeFingerSwipe)
            } else {
                None
            }
        }
        4 => {
            let thumb_count = current
                .iter()
                .filter(|contact| contact.role == Some(ContactRole::Thumb))
                .count();
            if thumb_count == 1 && m.span_delta.abs() >= config.pinch_commit_mm {
                Some(if m.span_delta < 0.0 {
                    ContinuousGestureKind::ThumbThreePinch
                } else {
                    ContinuousGestureKind::ThumbThreeSpread
                })
            } else if state.edge_started && translation_norm >= config.edge_commit_mm {
                Some(ContinuousGestureKind::EdgeSwipe)
            } else if translation_norm >= config.multi_swipe_commit_mm {
                Some(ContinuousGestureKind::FourFingerSwipe)
            } else {
                None
            }
        }
        _ => None,
    };

    if let Some(kind) = kind {
        state.committed = Some(kind);
        GestureDecision {
            events: vec![event(
                kind,
                ContinuousGesturePhase::Begin,
                m.translation.0,
                m.translation.1,
                m.scale,
                m.rotation,
            )],
            blocks_contact_policy: true,
        }
    } else {
        GestureDecision::default()
    }
}

#[derive(Clone, Copy)]
struct Metrics {
    translation: (f64, f64),
    span_delta: f64,
    scale: f64,
    rotation: f64,
}

fn metrics(anchor: &[GestureContact], current: &[GestureContact]) -> Metrics {
    let ac = centroid(anchor);
    let cc = centroid(current);
    let ar = mean_radius(anchor, ac);
    let cr = mean_radius(current, cc);
    let rotation = if anchor.len() == 2 {
        let aa = (anchor[1].y_mm - anchor[0].y_mm).atan2(anchor[1].x_mm - anchor[0].x_mm);
        let ca = (current[1].y_mm - current[0].y_mm).atan2(current[1].x_mm - current[0].x_mm);
        normalize_angle(ca - aa)
    } else {
        0.0
    };
    Metrics {
        translation: (cc.0 - ac.0, cc.1 - ac.1),
        span_delta: cr - ar,
        scale: if ar > 0.0 { cr / ar } else { 1.0 },
        rotation,
    }
}

fn centroid(contacts: &[GestureContact]) -> (f64, f64) {
    let n = contacts.len() as f64;
    (
        contacts.iter().map(|c| c.x_mm).sum::<f64>() / n,
        contacts.iter().map(|c| c.y_mm).sum::<f64>() / n,
    )
}

fn mean_radius(contacts: &[GestureContact], center: (f64, f64)) -> f64 {
    contacts
        .iter()
        .map(|c| (c.x_mm - center.0).hypot(c.y_mm - center.1))
        .sum::<f64>()
        / contacts.len() as f64
}

fn same_ids(anchor: &[GestureContact], current: &[GestureContact]) -> bool {
    anchor.len() == current.len()
        && anchor
            .iter()
            .zip(current)
            .all(|(a, c)| a.tracking_id == c.tracking_id)
}

fn edge_started(config: &GestureConfig, contacts: &[GestureContact]) -> bool {
    let Some((width, height)) = config.surface_size_mm else {
        return false;
    };
    contacts.iter().any(|contact| {
        contact.x_mm <= config.edge_zone_mm
            || contact.y_mm <= config.edge_zone_mm
            || contact.x_mm >= width - config.edge_zone_mm
            || contact.y_mm >= height - config.edge_zone_mm
    })
}

fn normalize_angle(mut angle: f64) -> f64 {
    while angle > std::f64::consts::PI {
        angle -= std::f64::consts::TAU;
    }
    while angle < -std::f64::consts::PI {
        angle += std::f64::consts::TAU;
    }
    angle
}

fn event(
    kind: ContinuousGestureKind,
    phase: ContinuousGesturePhase,
    x: f64,
    y: f64,
    scale: f64,
    rotation: f64,
) -> ContinuousGestureEvent {
    ContinuousGestureEvent {
        kind,
        phase,
        translation_x_mm: x as f32,
        translation_y_mm: y as f32,
        scale: scale as f32,
        rotation_radians: rotation as f32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> GestureConfig {
        GestureConfig::new(0.8, 0.15, 0.8, 4.0, 1.0, 2.0, 2.0, 3.0).unwrap()
    }
    fn c(id: i32, x: f64, y: f64) -> GestureContact {
        GestureContact {
            tracking_id: id,
            x_mm: x,
            y_mm: y,
            role: None,
        }
    }

    #[test]
    fn pinch_commits_before_translation_scroll_threshold() {
        let mut state = GestureState::default();
        let _ = process_gesture(&cfg(), &mut state, &[c(1, 0.0, 0.0), c(2, 10.0, 0.0)]);
        let d = process_gesture(&cfg(), &mut state, &[c(1, -1.0, 0.0), c(2, 11.0, 0.0)]);
        assert!(d.blocks_contact_policy);
        assert_eq!(d.events[0].kind, ContinuousGestureKind::Pinch);
    }

    #[test]
    fn ordinary_translation_yields_to_scroll() {
        let mut state = GestureState::default();
        let _ = process_gesture(&cfg(), &mut state, &[c(1, 0.0, 0.0), c(2, 10.0, 0.0)]);
        let d = process_gesture(&cfg(), &mut state, &[c(1, 0.0, 2.0), c(2, 10.0, 2.0)]);
        assert!(!d.blocks_contact_policy);
        assert_eq!(state.committed(), None);
    }

    #[test]
    fn three_finger_swipe_begin_update_end() {
        let mut state = GestureState::default();
        let anchor = [c(1, 0.0, 0.0), c(2, 5.0, 0.0), c(3, 10.0, 0.0)];
        let _ = process_gesture(&cfg(), &mut state, &anchor);
        let begin = process_gesture(
            &cfg(),
            &mut state,
            &[c(1, 3.0, 0.0), c(2, 8.0, 0.0), c(3, 13.0, 0.0)],
        );
        assert_eq!(begin.events[0].phase, ContinuousGesturePhase::Begin);
        let update = process_gesture(
            &cfg(),
            &mut state,
            &[c(1, 4.0, 0.0), c(2, 9.0, 0.0), c(3, 14.0, 0.0)],
        );
        assert_eq!(update.events[0].phase, ContinuousGesturePhase::Update);
        let end = process_gesture(&cfg(), &mut state, &[]);
        assert_eq!(end.events[0].phase, ContinuousGesturePhase::End);
    }

    #[test]
    fn thumb_three_requires_explicit_thumb_metadata() {
        let mut state = GestureState::default();
        let mut anchor = vec![
            c(1, 0.0, 0.0),
            c(2, 4.0, 0.0),
            c(3, 8.0, 0.0),
            c(4, 12.0, 0.0),
        ];
        anchor[0].role = Some(ContactRole::Thumb);
        let _ = process_gesture(&cfg(), &mut state, &anchor);
        let mut moved = vec![
            c(1, 2.0, 0.0),
            c(2, 5.0, 0.0),
            c(3, 7.0, 0.0),
            c(4, 10.0, 0.0),
        ];
        moved[0].role = Some(ContactRole::Thumb);
        let d = process_gesture(&cfg(), &mut state, &moved);
        assert_eq!(d.events[0].kind, ContinuousGestureKind::ThumbThreePinch);
    }
}
