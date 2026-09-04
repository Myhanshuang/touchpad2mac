//! Precision Touchpad report assembly into the shared contact model.
//!
//! Win32/HID parsing itself lives in the Windows FFI boundary. This module is
//! pure Rust so hybrid-report assembly, contact-id lifetime tracking and HID
//! physical-unit conversion remain testable on Linux CI as well.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

use touchpad_core::{Contact, ContactFrame, ContactState, Millimeters, Monotonic};

use crate::WindowsError;

pub(crate) const MAX_PTP_CONTACTS: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct AxisCalibration {
    pub logical_min: i32,
    pub logical_max: i32,
    pub physical_min: i32,
    pub physical_max: i32,
    pub units: u32,
    pub units_exp: u32,
}

impl AxisCalibration {
    pub(crate) fn to_mm(self, raw: u32) -> Result<f32, WindowsError> {
        if self.logical_max <= self.logical_min || self.physical_max == self.physical_min {
            return Err(WindowsError::Decode(
                "PTP axis has no usable logical/physical range".to_string(),
            ));
        }
        let logical = if self.logical_min < 0 {
            raw as i32 as f64
        } else {
            f64::from(raw)
        };
        let fraction = (logical - f64::from(self.logical_min))
            / f64::from(self.logical_max - self.logical_min);
        let physical = f64::from(self.physical_min)
            + fraction * f64::from(self.physical_max - self.physical_min);

        // USB HID unit encoding: low nibble is the unit system; the next
        // nibble is the length exponent. PTP X/Y are length quantities.
        // SI Linear uses centimetres, English Linear uses inches.
        let system = self.units & 0x0f;
        let length_power = ((self.units >> 4) & 0x0f) as i8;
        if length_power != 1 {
            return Err(WindowsError::Decode(format!(
                "PTP axis HID unit does not describe length (units={:#x})",
                self.units
            )));
        }
        let unit_mm = match system {
            1 => 10.0_f64,
            3 => 25.4_f64,
            other => {
                return Err(WindowsError::Decode(format!(
                    "PTP axis uses unsupported HID unit system {other} (units={:#x})",
                    self.units
                )))
            }
        };
        let exponent = signed_hid_exponent(self.units_exp);
        let mm = physical * 10_f64.powi(exponent) * unit_mm;
        if mm < f64::from(f32::MIN) || mm > f64::from(f32::MAX) {
            return Err(WindowsError::Decode(
                "PTP physical coordinate is outside f32 range".to_string(),
            ));
        }
        let mm = mm as f32;
        if !mm.is_finite() {
            return Err(WindowsError::Decode(
                "PTP physical coordinate is non-finite".to_string(),
            ));
        }
        Ok(mm)
    }

    pub(crate) fn normalized(self, raw: u32) -> Option<f32> {
        if self.logical_max <= self.logical_min {
            return None;
        }
        let logical = if self.logical_min < 0 {
            raw as i32 as f64
        } else {
            f64::from(raw)
        };
        let value = ((logical - f64::from(self.logical_min))
            / f64::from(self.logical_max - self.logical_min))
        .clamp(0.0, 1.0);
        Some(value as f32)
    }
}

fn signed_hid_exponent(raw: u32) -> i32 {
    let nibble = (raw & 0x0f) as i32;
    if nibble >= 8 {
        nibble - 16
    } else {
        nibble
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RawPtpContact {
    pub id: u32,
    pub tip: bool,
    pub x_mm: f32,
    pub y_mm: f32,
    pub pressure: Option<f32>,
    pub width_mm: Option<f32>,
    pub height_mm: Option<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DecodedPtpReport {
    pub scan_time: u32,
    pub contact_count: usize,
    pub contacts: Vec<RawPtpContact>,
}

#[derive(Clone, Debug)]
struct PendingFrame {
    scan_time: u32,
    expected_contacts: usize,
    contacts: BTreeMap<u32, RawPtpContact>,
}

#[derive(Clone, Debug)]
struct ActiveContact {
    slot: u32,
    x_mm: f32,
    y_mm: f32,
    pressure: Option<f32>,
    width_mm: Option<f32>,
    height_mm: Option<f32>,
}

/// Assembles one or more hybrid HID reports with the same Scan Time into one
/// contact frame and translates PTP Contact IDs into stable core slots.
#[derive(Clone, Debug, Default)]
pub(crate) struct PtpFrameAssembler {
    pending: Option<PendingFrame>,
    active: BTreeMap<u32, ActiveContact>,
    next_sequence: u64,
}

impl PtpFrameAssembler {
    pub(crate) fn push_report(
        &mut self,
        report: DecodedPtpReport,
        timestamp: Monotonic,
    ) -> Result<Vec<ContactFrame>, WindowsError> {
        if report.contact_count > MAX_PTP_CONTACTS || report.contacts.len() > MAX_PTP_CONTACTS {
            return Err(WindowsError::Decode(format!(
                "PTP report exceeds the Windows maximum of {MAX_PTP_CONTACTS} contacts"
            )));
        }

        let mut frames = Vec::new();
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.scan_time != report.scan_time)
        {
            // The previous hybrid frame never reached its advertised contact
            // count. Flush it as a discontinuity instead of merging reports
            // across scan boundaries.
            if let Some(pending) = self.pending.take() {
                frames.push(self.finish_pending(pending, timestamp, true)?);
            }
        }

        if report.contact_count == 0 && self.pending.is_none() && report.contacts.is_empty() {
            let pending = PendingFrame {
                scan_time: report.scan_time,
                expected_contacts: 0,
                contacts: BTreeMap::new(),
            };
            frames.push(self.finish_pending(pending, timestamp, false)?);
            return Ok(frames);
        }

        // A non-zero Contact Count starts a normal or hybrid frame. A zero
        // count is only a continuation when a same-Scan-Time frame is already
        // pending. If Raw Input exposes a continuation without its starter,
        // preserve the contacts but commit the recovered frame discontinuous.
        let continuation_without_start = self.pending.is_none() && report.contact_count == 0;
        let pending = self.pending.get_or_insert_with(|| PendingFrame {
            scan_time: report.scan_time,
            expected_contacts: if report.contact_count == 0 {
                report.contacts.len()
            } else {
                report.contact_count
            },
            contacts: BTreeMap::new(),
        });
        if report.contact_count > 0 {
            pending.expected_contacts = report.contact_count;
        }
        for contact in report.contacts {
            pending.contacts.insert(contact.id, contact);
        }

        if pending.contacts.len() >= pending.expected_contacts {
            let pending = self.pending.take().expect("pending exists");
            frames.push(self.finish_pending(pending, timestamp, continuation_without_start)?);
        }
        Ok(frames)
    }

    fn finish_pending(
        &mut self,
        pending: PendingFrame,
        timestamp: Monotonic,
        discontinuity: bool,
    ) -> Result<ContactFrame, WindowsError> {
        self.next_sequence = self.next_sequence.saturating_add(1);
        let mut frame = if discontinuity {
            self.active.clear();
            ContactFrame::with_discontinuity(timestamp, self.next_sequence)
        } else {
            ContactFrame::new(timestamp, self.next_sequence)
        };

        let reported_ids: BTreeSet<u32> = pending.contacts.keys().copied().collect();
        let mut ended_after_frame = Vec::new();

        for raw in pending.contacts.values() {
            if raw.tip {
                let state = if self.active.contains_key(&raw.id) {
                    ContactState::Active
                } else {
                    ContactState::Began
                };
                let slot = match self.active.get(&raw.id) {
                    Some(active) => active.slot,
                    None => self.allocate_slot()?,
                };
                let active = ActiveContact {
                    slot,
                    x_mm: raw.x_mm,
                    y_mm: raw.y_mm,
                    pressure: raw.pressure,
                    width_mm: raw.width_mm,
                    height_mm: raw.height_mm,
                };
                self.active.insert(raw.id, active.clone());
                frame.contacts.push(core_contact(raw.id, state, &active)?);
            } else if let Some(active) = self.active.get(&raw.id).cloned() {
                let ended = ActiveContact {
                    x_mm: raw.x_mm,
                    y_mm: raw.y_mm,
                    pressure: raw.pressure.or(active.pressure),
                    width_mm: raw.width_mm.or(active.width_mm),
                    height_mm: raw.height_mm.or(active.height_mm),
                    ..active
                };
                frame
                    .contacts
                    .push(core_contact(raw.id, ContactState::Ended, &ended)?);
                ended_after_frame.push(raw.id);
            }
        }

        if !discontinuity {
            // A completed PTP frame describes the complete contact set. End
            // any previously-live ID omitted by firmware so the shared core
            // can never retain a stuck contact indefinitely.
            for (&id, active) in &self.active {
                if !reported_ids.contains(&id) {
                    frame
                        .contacts
                        .push(core_contact(id, ContactState::Ended, active)?);
                    ended_after_frame.push(id);
                }
            }
        }

        for id in ended_after_frame {
            self.active.remove(&id);
        }
        frame.contacts.sort_by_key(|contact| contact.slot);
        Ok(frame)
    }

    fn allocate_slot(&self) -> Result<u32, WindowsError> {
        let used: BTreeSet<u32> = self.active.values().map(|contact| contact.slot).collect();
        (0..MAX_PTP_CONTACTS as u32)
            .find(|slot| !used.contains(slot))
            .ok_or_else(|| WindowsError::Decode("no free PTP contact slot".to_string()))
    }
}

fn core_contact(
    id: u32,
    state: ContactState,
    source: &ActiveContact,
) -> Result<Contact, WindowsError> {
    let tracking_id = i32::try_from(id)
        .map_err(|_| WindowsError::Decode(format!("PTP contact id {id} exceeds i32")))?;
    let mut contact = Contact::new(tracking_id, source.slot, state);
    contact.x_mm = Some(
        Millimeters::try_new(source.x_mm)
            .map_err(|error| WindowsError::Decode(format!("invalid PTP X coordinate: {error}")))?,
    );
    contact.y_mm = Some(
        Millimeters::try_new(source.y_mm)
            .map_err(|error| WindowsError::Decode(format!("invalid PTP Y coordinate: {error}")))?,
    );
    contact.pressure = source.pressure;
    contact.major_mm = source
        .width_mm
        .map(Millimeters::try_new)
        .transpose()
        .map_err(|error| WindowsError::Decode(format!("invalid PTP width: {error}")))?;
    contact.minor_mm = source
        .height_mm
        .map(Millimeters::try_new)
        .transpose()
        .map_err(|error| WindowsError::Decode(format!("invalid PTP height: {error}")))?;
    Ok(contact)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(id: u32, tip: bool, x: f32) -> RawPtpContact {
        RawPtpContact {
            id,
            tip,
            x_mm: x,
            y_mm: 20.0,
            pressure: Some(0.5),
            width_mm: Some(5.0),
            height_mm: Some(4.0),
        }
    }

    #[test]
    fn hid_si_linear_coordinate_converts_to_millimeters() {
        let scale = AxisCalibration {
            logical_min: 0,
            logical_max: 1000,
            physical_min: 0,
            physical_max: 1000,
            // SI Linear + length^1; exponent -2 -> hundredths of centimetre.
            units: 0x11,
            units_exp: 0x0e,
        };
        assert!((scale.to_mm(500).unwrap() - 50.0).abs() < 0.001);
        assert!((scale.normalized(500).unwrap() - 0.5).abs() < 0.001);
    }

    #[test]
    fn hybrid_reports_commit_only_after_advertised_contact_count() {
        let mut assembler = PtpFrameAssembler::default();
        let first = assembler
            .push_report(
                DecodedPtpReport {
                    scan_time: 7,
                    contact_count: 3,
                    contacts: vec![raw(10, true, 10.0), raw(11, true, 20.0)],
                },
                Monotonic::from_nanos(1),
            )
            .unwrap();
        assert!(first.is_empty());
        let second = assembler
            .push_report(
                DecodedPtpReport {
                    scan_time: 7,
                    contact_count: 0,
                    contacts: vec![raw(12, true, 30.0)],
                },
                Monotonic::from_nanos(2),
            )
            .unwrap();
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].contacts.len(), 3);
        assert!(second[0]
            .contacts
            .iter()
            .all(|contact| contact.state == ContactState::Began));
    }

    #[test]
    fn omitted_live_contact_is_ended_on_complete_next_frame() {
        let mut assembler = PtpFrameAssembler::default();
        let began = assembler
            .push_report(
                DecodedPtpReport {
                    scan_time: 1,
                    contact_count: 1,
                    contacts: vec![raw(4, true, 10.0)],
                },
                Monotonic::from_nanos(1),
            )
            .unwrap();
        assert_eq!(began[0].contacts[0].state, ContactState::Began);

        let ended = assembler
            .push_report(
                DecodedPtpReport {
                    scan_time: 2,
                    contact_count: 0,
                    contacts: Vec::new(),
                },
                Monotonic::from_nanos(2),
            )
            .unwrap();
        assert_eq!(ended[0].contacts.len(), 1);
        assert_eq!(ended[0].contacts[0].state, ContactState::Ended);
    }
}
