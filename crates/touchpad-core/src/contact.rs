//! Contact and frame model (design.md §5, IMPLEMENTATION_BRIEF §4).
//!
//! A [`ContactFrame`] is the normalized unit of input published by the
//! platform input layer once per kernel `SYN_REPORT` boundary. All
//! interaction algorithms consume frames; they never see raw kernel events.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::diagnostic::{Diagnostic, DiagnosticCode, DiagnosticLevel};
use crate::time::Monotonic;
use crate::units::Millimeters;

/// Lifecycle state of a contact within a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ContactState {
    /// A new tracking id appeared in this frame.
    Began,
    /// The contact persisted from an earlier frame.
    Active,
    /// The contact ended in this frame.
    Ended,
}

/// Physical button state captured with a frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PhysicalButtons {
    pub left: bool,
    pub right: bool,
    pub middle: bool,
}

impl PhysicalButtons {
    /// No physical button pressed.
    pub const NONE: PhysicalButtons = PhysicalButtons {
        left: false,
        right: false,
        middle: false,
    };

    /// Creates a button state from the three physical buttons.
    #[must_use]
    pub const fn new(left: bool, right: bool, middle: bool) -> Self {
        Self {
            left,
            right,
            middle,
        }
    }

    /// Whether no physical button is pressed.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        !self.left && !self.right && !self.middle
    }
}

/// A single touch contact in a committed frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Contact {
    /// Device tracking id; `>= 0` for live contacts.
    pub tracking_id: i32,
    /// Type-B slot index this contact occupies.
    pub slot: u32,
    /// Physical position in millimeters.
    ///
    /// `None` only for contacts whose coordinates are not yet known; the
    /// decoder must not publish a `Began` contact until both coordinates are
    /// available ([`Contact::validate`] flags violations).
    pub x_mm: Option<Millimeters>,
    /// Physical position in millimeters (see [`Contact::x_mm`]).
    pub y_mm: Option<Millimeters>,
    /// Normalized pressure in `[0, 1]`, when the device reports it.
    pub pressure: Option<f32>,
    /// Contact ellipse major axis in millimeters, when reported.
    pub major_mm: Option<Millimeters>,
    /// Contact ellipse minor axis in millimeters, when reported.
    pub minor_mm: Option<Millimeters>,
    /// Contact orientation in radians, when reported.
    pub orientation: Option<f32>,
    /// Lifecycle state within this frame.
    pub state: ContactState,
}

impl Contact {
    /// Creates a contact with no optional data filled in.
    #[must_use]
    pub fn new(tracking_id: i32, slot: u32, state: ContactState) -> Self {
        Self {
            tracking_id,
            slot,
            state,
            x_mm: None,
            y_mm: None,
            pressure: None,
            major_mm: None,
            minor_mm: None,
            orientation: None,
        }
    }

    /// Whether both coordinates are known.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.x_mm.is_some() && self.y_mm.is_some()
    }

    /// Structural validation; returns diagnostics instead of panicking.
    ///
    /// Checks: live contacts carry a non-negative tracking id, `Began`
    /// contacts have both coordinates, pressure is a finite value in
    /// `[0, 1]`, orientation is finite, and ellipse axes are non-negative.
    #[must_use]
    pub fn validate(&self) -> Vec<Diagnostic> {
        let mut out = Vec::new();

        match self.state {
            ContactState::Began | ContactState::Active => {
                if self.tracking_id < 0 {
                    out.push(Diagnostic::new(
                        DiagnosticLevel::Error,
                        DiagnosticCode::InvalidEventOrder,
                        format!(
                            "live contact on slot {} has negative tracking id {}",
                            self.slot, self.tracking_id
                        ),
                    ));
                }
                if self.state == ContactState::Began && !self.is_complete() {
                    out.push(Diagnostic::new(
                        DiagnosticLevel::Warning,
                        DiagnosticCode::IncompleteNewContact,
                        format!(
                            "Began contact on slot {} has missing coordinates",
                            self.slot
                        ),
                    ));
                }
            }
            ContactState::Ended => {
                if self.tracking_id < 0 {
                    out.push(Diagnostic::new(
                        DiagnosticLevel::Error,
                        DiagnosticCode::InvalidEventOrder,
                        format!("Ended contact on slot {} carries no tracking id", self.slot),
                    ));
                }
            }
        }

        if let Some(pressure) = self.pressure {
            if !pressure.is_finite() {
                out.push(Diagnostic::new(
                    DiagnosticLevel::Error,
                    DiagnosticCode::NonFiniteValue,
                    "pressure must be finite",
                ));
            } else if !(0.0..=1.0).contains(&pressure) {
                out.push(Diagnostic::new(
                    DiagnosticLevel::Error,
                    DiagnosticCode::OutOfRangeValue,
                    format!("pressure {pressure} is outside [0, 1]"),
                ));
            }
        }

        if let Some(orientation) = self.orientation {
            if !orientation.is_finite() {
                out.push(Diagnostic::new(
                    DiagnosticLevel::Error,
                    DiagnosticCode::NonFiniteValue,
                    "orientation must be finite",
                ));
            }
        }

        for (name, value) in [("major_mm", self.major_mm), ("minor_mm", self.minor_mm)] {
            if let Some(mm) = value {
                if mm.as_mm() < 0.0 {
                    out.push(Diagnostic::new(
                        DiagnosticLevel::Error,
                        DiagnosticCode::OutOfRangeValue,
                        format!("{name} must be non-negative"),
                    ));
                }
            }
        }

        out
    }
}

/// A committed frame of input state, published once per kernel `SYN_REPORT`
/// boundary (or replay equivalent).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContactFrame {
    /// Monotonic timestamp of the frame.
    pub monotonic_timestamp: Monotonic,
    /// Monotonically increasing frame sequence.
    pub sequence: u64,
    /// True when the input stream lost continuity and was resynchronized
    /// (e.g. after `SYN_DROPPED`). Consumers must not compare this frame's
    /// contacts with the previous frame.
    pub discontinuity: bool,
    /// Contacts in this frame.
    pub contacts: Vec<Contact>,
    /// Physical button state as of this frame.
    pub physical_buttons: PhysicalButtons,
    /// Diagnostics attached to this frame (decoder warnings, recovery
    /// notices, validation findings).
    pub diagnostics: Vec<Diagnostic>,
}

impl ContactFrame {
    /// Creates an empty, continuous frame.
    #[must_use]
    pub fn new(monotonic_timestamp: Monotonic, sequence: u64) -> Self {
        Self {
            monotonic_timestamp,
            sequence,
            discontinuity: false,
            contacts: Vec::new(),
            physical_buttons: PhysicalButtons::NONE,
            diagnostics: Vec::new(),
        }
    }

    /// Creates an empty frame flagged as discontinuous (after resync).
    #[must_use]
    pub fn with_discontinuity(monotonic_timestamp: Monotonic, sequence: u64) -> Self {
        let mut frame = Self::new(monotonic_timestamp, sequence);
        frame.discontinuity = true;
        frame
    }

    /// Validates frame structure: no duplicate slots, plus per-contact
    /// validation. Returns diagnostics instead of panicking.
    #[must_use]
    pub fn validate(&self) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for contact in &self.contacts {
            if !seen.insert(contact.slot) {
                out.push(Diagnostic::new(
                    DiagnosticLevel::Error,
                    DiagnosticCode::DuplicateSlot,
                    format!("duplicate slot {} in frame {}", contact.slot, self.sequence),
                ));
            }
            out.extend(contact.validate());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::Monotonic;

    fn completed_contact(slot: u32) -> Contact {
        let mut contact = Contact::new(42, slot, ContactState::Began);
        contact.x_mm = Some(Millimeters::try_new(10.0).unwrap());
        contact.y_mm = Some(Millimeters::try_new(20.0).unwrap());
        contact
    }

    #[test]
    fn began_contact_without_coordinates_is_diagnosed() {
        let contact = Contact::new(1, 0, ContactState::Began);
        assert!(!contact.is_complete());
        let diags = contact.validate();
        assert!(diags
            .iter()
            .any(|d| d.code == DiagnosticCode::IncompleteNewContact));
    }

    #[test]
    fn complete_began_contact_validates_clean() {
        let contact = completed_contact(0);
        assert!(contact.is_complete());
        assert!(contact.validate().is_empty());
    }

    #[test]
    fn invalid_pressure_is_diagnosed() {
        let mut contact = completed_contact(0);
        contact.pressure = Some(1.5);
        assert!(contact
            .validate()
            .iter()
            .any(|d| d.code == DiagnosticCode::OutOfRangeValue));
        contact.pressure = Some(f32::NAN);
        assert!(contact
            .validate()
            .iter()
            .any(|d| d.code == DiagnosticCode::NonFiniteValue));
    }

    #[test]
    fn negative_live_tracking_id_is_diagnosed() {
        let contact = Contact::new(-1, 0, ContactState::Active);
        assert!(contact
            .validate()
            .iter()
            .any(|d| d.code == DiagnosticCode::InvalidEventOrder));
    }

    #[test]
    fn frame_rejects_duplicate_slots() {
        let mut frame = ContactFrame::new(Monotonic::ZERO, 0);
        frame.contacts.push(completed_contact(0));
        frame.contacts.push(completed_contact(0));
        let diags = frame.validate();
        assert!(diags
            .iter()
            .any(|d| d.code == DiagnosticCode::DuplicateSlot));
    }

    #[test]
    fn frame_discontinuity_flag() {
        assert!(!ContactFrame::new(Monotonic::ZERO, 0).discontinuity);
        assert!(ContactFrame::with_discontinuity(Monotonic::ZERO, 1).discontinuity);
    }
}
