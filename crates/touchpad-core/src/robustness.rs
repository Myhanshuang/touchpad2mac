//! M13 contact robustness: explicit feature-aware classification, sticky
//! suppression and bounded contact-position jitter filtering.

#![forbid(unsafe_code)]

use std::time::Duration;

use crate::{Contact, ContactFrame, ContactState, Millimeters, Monotonic};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContactRole {
    Finger,
    Thumb,
    Palm,
    EdgeSuppressed,
    TypingSuppressed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RobustnessAvailability {
    pub contact_size: bool,
    pub pressure: bool,
    pub orientation: bool,
    pub edge_geometry: bool,
    pub typing_signal: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RobustnessConfig {
    palm_major_mm: f64,
    thumb_major_mm: f64,
    edge_zone_mm: f64,
    jitter_radius_mm: f64,
    typing_suppression: Duration,
    surface_size_mm: Option<(f64, f64)>,
}

impl RobustnessConfig {
    pub fn new(
        palm_major_mm: f64,
        thumb_major_mm: f64,
        edge_zone_mm: f64,
        jitter_radius_mm: f64,
        typing_suppression: Duration,
    ) -> Result<Self, RobustnessConfigError> {
        for (name, value) in [
            ("palm_major_mm", palm_major_mm),
            ("thumb_major_mm", thumb_major_mm),
            ("edge_zone_mm", edge_zone_mm),
            ("jitter_radius_mm", jitter_radius_mm),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(RobustnessConfigError::NonPositiveOrNonFinite { name, value });
            }
        }
        if palm_major_mm <= thumb_major_mm {
            return Err(RobustnessConfigError::InvalidOrdering(
                "palm_major_mm must be greater than thumb_major_mm",
            ));
        }
        if typing_suppression.is_zero() {
            return Err(RobustnessConfigError::ZeroTypingSuppression);
        }
        Ok(Self {
            palm_major_mm,
            thumb_major_mm,
            edge_zone_mm,
            jitter_radius_mm,
            typing_suppression,
            surface_size_mm: None,
        })
    }

    pub fn with_surface_size_mm(
        mut self,
        width_mm: f64,
        height_mm: f64,
    ) -> Result<Self, RobustnessConfigError> {
        if !width_mm.is_finite() || !height_mm.is_finite() || width_mm <= 0.0 || height_mm <= 0.0 {
            return Err(RobustnessConfigError::InvalidSurface);
        }
        if width_mm <= self.edge_zone_mm * 2.0 || height_mm <= self.edge_zone_mm * 2.0 {
            return Err(RobustnessConfigError::InvalidSurface);
        }
        self.surface_size_mm = Some((width_mm, height_mm));
        Ok(self)
    }

    #[must_use]
    pub const fn palm_major_mm(&self) -> f64 {
        self.palm_major_mm
    }
    #[must_use]
    pub const fn thumb_major_mm(&self) -> f64 {
        self.thumb_major_mm
    }
    #[must_use]
    pub const fn edge_zone_mm(&self) -> f64 {
        self.edge_zone_mm
    }
    #[must_use]
    pub const fn jitter_radius_mm(&self) -> f64 {
        self.jitter_radius_mm
    }
    #[must_use]
    pub const fn typing_suppression(&self) -> Duration {
        self.typing_suppression
    }
    #[must_use]
    pub const fn surface_size_mm(&self) -> Option<(f64, f64)> {
        self.surface_size_mm
    }
}

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum RobustnessConfigError {
    #[error("M13 robustness requires finite positive {name}, got {value}")]
    NonPositiveOrNonFinite { name: &'static str, value: f64 },
    #[error("invalid M13 robustness ordering: {0}")]
    InvalidOrdering(&'static str),
    #[error("typing suppression duration must be non-zero")]
    ZeroTypingSuppression,
    #[error("invalid touch-surface dimensions for edge suppression")]
    InvalidSurface,
}

#[derive(Clone, Debug, PartialEq)]
struct TrackedContact {
    tracking_id: i32,
    role: ContactRole,
    last_filtered: Option<(f64, f64)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RobustnessState {
    tracked: Vec<TrackedContact>,
    last_typing: Option<Monotonic>,
    typing_signal_seen: bool,
}

impl RobustnessState {
    pub fn note_typing(&mut self, timestamp: Monotonic) {
        self.last_typing = Some(timestamp);
        self.typing_signal_seen = true;
    }

    #[must_use]
    pub fn role(&self, tracking_id: i32) -> Option<ContactRole> {
        self.tracked
            .iter()
            .find(|tracked| tracked.tracking_id == tracking_id)
            .map(|tracked| tracked.role)
    }

    pub fn clear(&mut self) {
        self.tracked.clear();
    }

    #[must_use]
    pub const fn typing_signal_seen(&self) -> bool {
        self.typing_signal_seen
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RobustnessOutcome {
    pub frame: ContactFrame,
    pub availability: RobustnessAvailability,
    pub classified: Vec<(i32, ContactRole)>,
}

pub fn filter_frame(
    config: &RobustnessConfig,
    state: &mut RobustnessState,
    frame: &ContactFrame,
) -> RobustnessOutcome {
    if frame.discontinuity {
        state.clear();
    }
    let mut availability = RobustnessAvailability {
        edge_geometry: config.surface_size_mm.is_some(),
        typing_signal: state.typing_signal_seen,
        ..RobustnessAvailability::default()
    };
    let mut output = frame.clone();
    output.contacts.clear();
    let mut classified = Vec::new();

    for source in &frame.contacts {
        availability.contact_size |= source.major_mm.is_some() || source.minor_mm.is_some();
        availability.pressure |= source.pressure.is_some();
        availability.orientation |= source.orientation.is_some();

        if source.state == ContactState::Ended {
            if let Some(index) = state
                .tracked
                .iter()
                .position(|tracked| tracked.tracking_id == source.tracking_id)
            {
                let tracked = state.tracked.remove(index);
                classified.push((source.tracking_id, tracked.role));
                if !is_suppressed(tracked.role) {
                    output.contacts.push(source.clone());
                }
            } else {
                output.contacts.push(source.clone());
            }
            continue;
        }

        if let Some(index) = state
            .tracked
            .iter()
            .position(|tracked| tracked.tracking_id == source.tracking_id)
        {
            let tracked = &mut state.tracked[index];
            classified.push((source.tracking_id, tracked.role));
            if !is_suppressed(tracked.role) {
                output.contacts.push(jitter_filter(config, tracked, source));
            }
            continue;
        }

        // An unseen Active contact (e.g. after recovery) is classified from
        // the available evidence but cannot claim a known edge-start history.
        let role = classify_new(config, state, source, frame.monotonic_timestamp);
        let mut tracked = TrackedContact {
            tracking_id: source.tracking_id,
            role,
            last_filtered: position(source),
        };
        classified.push((source.tracking_id, role));
        if !is_suppressed(role) {
            output
                .contacts
                .push(jitter_filter(config, &mut tracked, source));
        }
        state.tracked.push(tracked);
    }

    RobustnessOutcome {
        frame: output,
        availability,
        classified,
    }
}

fn classify_new(
    config: &RobustnessConfig,
    state: &RobustnessState,
    contact: &Contact,
    timestamp: Monotonic,
) -> ContactRole {
    if typing_active(config, state, timestamp) {
        return ContactRole::TypingSuppressed;
    }
    if contact.state == ContactState::Began && edge_start(config, contact) {
        return ContactRole::EdgeSuppressed;
    }
    if let Some(major) = contact.major_mm.map(|v| f64::from(v.as_mm())) {
        if major >= config.palm_major_mm {
            return ContactRole::Palm;
        }
        if major >= config.thumb_major_mm {
            return ContactRole::Thumb;
        }
    }
    ContactRole::Finger
}

fn typing_active(config: &RobustnessConfig, state: &RobustnessState, now: Monotonic) -> bool {
    let Some(last) = state.last_typing else {
        return false;
    };
    now.duration_since(last)
        .is_some_and(|elapsed| elapsed <= config.typing_suppression)
}

fn edge_start(config: &RobustnessConfig, contact: &Contact) -> bool {
    let (Some((width, height)), Some((x, y))) = (config.surface_size_mm, position(contact)) else {
        return false;
    };
    x <= config.edge_zone_mm
        || y <= config.edge_zone_mm
        || x >= width - config.edge_zone_mm
        || y >= height - config.edge_zone_mm
}

fn jitter_filter(
    config: &RobustnessConfig,
    tracked: &mut TrackedContact,
    source: &Contact,
) -> Contact {
    let Some(current) = position(source) else {
        return source.clone();
    };
    let Some(last) = tracked.last_filtered else {
        tracked.last_filtered = Some(current);
        return source.clone();
    };
    let distance = (current.0 - last.0).hypot(current.1 - last.1);
    if distance < config.jitter_radius_mm {
        let mut filtered = source.clone();
        filtered.x_mm = Millimeters::try_new(last.0 as f32).ok();
        filtered.y_mm = Millimeters::try_new(last.1 as f32).ok();
        filtered
    } else {
        tracked.last_filtered = Some(current);
        source.clone()
    }
}

fn position(contact: &Contact) -> Option<(f64, f64)> {
    Some((
        f64::from(contact.x_mm?.as_mm()),
        f64::from(contact.y_mm?.as_mm()),
    ))
}

fn is_suppressed(role: ContactRole) -> bool {
    matches!(
        role,
        ContactRole::Palm | ContactRole::EdgeSuppressed | ContactRole::TypingSuppressed
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> RobustnessConfig {
        RobustnessConfig::new(12.0, 8.0, 3.0, 0.06, Duration::from_millis(500)).unwrap()
    }

    fn c(id: i32, state: ContactState, x: f32, y: f32, major: Option<f32>) -> Contact {
        let mut c = Contact::new(id, 0, state);
        c.x_mm = Some(Millimeters::try_new(x).unwrap());
        c.y_mm = Some(Millimeters::try_new(y).unwrap());
        c.major_mm = major.map(|v| Millimeters::try_new(v).unwrap());
        c
    }

    fn f(ts_ms: u64, contacts: Vec<Contact>) -> ContactFrame {
        ContactFrame {
            monotonic_timestamp: Monotonic::from_nanos(ts_ms * 1_000_000),
            sequence: ts_ms + 1,
            discontinuity: false,
            contacts,
            physical_buttons: crate::PhysicalButtons::NONE,
            diagnostics: vec![],
        }
    }

    #[test]
    fn missing_size_falls_back_to_finger() {
        let mut state = RobustnessState::default();
        let out = filter_frame(
            &cfg(),
            &mut state,
            &f(0, vec![c(1, ContactState::Began, 20.0, 20.0, None)]),
        );
        assert_eq!(out.classified, vec![(1, ContactRole::Finger)]);
        assert!(!out.availability.contact_size);
    }

    #[test]
    fn palm_is_suppressed_but_thumb_is_retained_metadata() {
        let mut state = RobustnessState::default();
        let out = filter_frame(
            &cfg(),
            &mut state,
            &f(
                0,
                vec![
                    c(1, ContactState::Began, 20.0, 20.0, Some(13.0)),
                    c(2, ContactState::Began, 30.0, 20.0, Some(9.0)),
                ],
            ),
        );
        assert_eq!(out.classified[0], (1, ContactRole::Palm));
        assert_eq!(out.classified[1], (2, ContactRole::Thumb));
        assert_eq!(out.frame.contacts.len(), 1);
        assert_eq!(out.frame.contacts[0].tracking_id, 2);
    }

    #[test]
    fn edge_start_is_sticky_until_end() {
        let config = cfg().with_surface_size_mm(131.0, 77.0).unwrap();
        let mut state = RobustnessState::default();
        let began = filter_frame(
            &config,
            &mut state,
            &f(0, vec![c(1, ContactState::Began, 1.0, 30.0, None)]),
        );
        assert!(began.frame.contacts.is_empty());
        let moved = filter_frame(
            &config,
            &mut state,
            &f(10, vec![c(1, ContactState::Active, 50.0, 30.0, None)]),
        );
        assert!(moved.frame.contacts.is_empty());
        let _ = filter_frame(
            &config,
            &mut state,
            &f(20, vec![c(1, ContactState::Ended, 50.0, 30.0, None)]),
        );
        assert_eq!(state.role(1), None);
    }

    #[test]
    fn typing_signal_suppresses_new_contact_for_window_only() {
        let mut state = RobustnessState::default();
        state.note_typing(Monotonic::ZERO);
        let suppressed = filter_frame(
            &cfg(),
            &mut state,
            &f(100, vec![c(1, ContactState::Began, 20.0, 20.0, None)]),
        );
        assert!(suppressed.frame.contacts.is_empty());
        let later = filter_frame(
            &cfg(),
            &mut state,
            &f(700, vec![c(2, ContactState::Began, 20.0, 20.0, None)]),
        );
        assert_eq!(later.frame.contacts.len(), 1);
    }

    #[test]
    fn jitter_holds_then_releases_real_position() {
        let mut state = RobustnessState::default();
        let _ = filter_frame(
            &cfg(),
            &mut state,
            &f(0, vec![c(1, ContactState::Began, 20.0, 20.0, None)]),
        );
        let held = filter_frame(
            &cfg(),
            &mut state,
            &f(10, vec![c(1, ContactState::Active, 20.03, 20.0, None)]),
        );
        assert_eq!(held.frame.contacts[0].x_mm.unwrap().as_mm(), 20.0);
        let released = filter_frame(
            &cfg(),
            &mut state,
            &f(20, vec![c(1, ContactState::Active, 20.1, 20.0, None)]),
        );
        assert_eq!(released.frame.contacts[0].x_mm.unwrap().as_mm(), 20.1);
    }
}
