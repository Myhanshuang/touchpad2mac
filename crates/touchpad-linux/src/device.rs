//! Linux input device enumeration and candidate touchpad probing (M4).
//!
//! [`probe`] opens a `/dev/input/event*` node through the [`Sys`] seam,
//! queries its identity, capabilities (`EVIOCGBIT` for `EV_KEY`/`EV_ABS`,
//! `EVIOCGPROP` for `INPUT_PROP_*`), per-axis limits (`EVIOCGABS`) and slot
//! count, and produces an explainable verdict:
//!
//! * [`ProbeVerdict::Candidate`] — a usable Type-B multitouch touchpad, with
//!   the [`DeviceDescriptor`] the decoder will be configured with;
//! * [`ProbeVerdict::Rejected`] — the device exists and was probed but does
//!   not qualify; `reasons` lists **each** failed check (M4 requirement 1:
//!   explainable rejection);
//! * [`ProbeVerdict::Inaccessible`] — the device could not be probed
//!   (permission, ioctl failure, ...), with an actionable error.
//!
//! The candidate rule (M4 requirement 1) is deliberately conservative and
//! fully documented:
//!
//! 1. **Type-B multitouch is required**: the device must report
//!    `ABS_MT_SLOT`, `ABS_MT_TRACKING_ID`, `ABS_MT_POSITION_X` and
//!    `ABS_MT_POSITION_Y` — exactly the axes the M3 decoder implements.
//! 2. **It must be an indirect pointer/buttonpad device**: either
//!    `INPUT_PROP_POINTER` or `INPUT_PROP_BUTTONPAD` must be set, and
//!    `INPUT_PROP_DIRECT` (touchscreen) disqualifies.
//! 3. **The slot count (from `ABS_MT_SLOT`'s `absinfo.max + 1`) must lie in
//!    `[1, MAX_SLOT_COUNT]`**, so the decoder's bounded per-slot state and
//!    the resync snapshot adapter can always be built.
//!
//! `EV_KEY` and `EV_ABS` are queried and recorded as evidence; physical
//! buttons (`BTN_LEFT/RIGHT/MIDDLE`) are noted and reflected in the
//! descriptor's `has_physical_buttons`, but their absence does not
//! disqualify a candidate (buttonpads may deliver clicks through contacts).
//!
//! ## Shared opened-fd probing (M4 review R4)
//!
//! The per-fd capability/axis/slot read lives in [`probe_open_fd`], which
//! operates on an **already-open** fd. Enumeration's temporary probe
//! ([`probe`]) opens its own handle and calls it; the runtime session open
//! ([`crate::runtime`]) validates the exact fd it will read from through the
//! same function, so the rules cannot drift and no device identity can be
//! swapped between a probe fd and a session fd.
//!
//! ## Response completeness (M4 review R7)
//!
//! Every required capability response is validated: a successful
//! `EVIOCGBIT`/`EVIOCGPROP` with a full-size buffer copies the whole kernel
//! bit array, so a shorter response means a truncated/mock-corrupt device
//! and fails as [`crate::sys::SysError::TruncatedResponse`] instead of being
//! silently treated as complete.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

use touchpad_core::{AxisInfo, DeviceDescriptor, DeviceProfile};

use crate::codes::{
    bits_to_bytes, test_bit, ABS_MAX, ABS_MT_POSITION_X, ABS_MT_POSITION_Y, ABS_MT_SLOT,
    ABS_MT_TRACKING_ID, BTN_LEFT, BTN_MIDDLE, BTN_RIGHT, EV_ABS, EV_KEY, EV_MAX,
    INPUT_PROP_BUTTONPAD, INPUT_PROP_DIRECT, INPUT_PROP_MAX, INPUT_PROP_POINTER, KEY_MAX,
};
use crate::sys::{AbsInfo, Fd, InputId, Sys, SysError};
use crate::MAX_SLOT_COUNT;

/// Failure of the top-level enumeration (probing an individual device never
/// fails — every outcome is a [`ProbeReport`]).
#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    /// The input directory itself could not be read (e.g. no `/dev/input` on
    /// the system, or the current user cannot read it).
    #[error("could not read input directory {path}: {source}")]
    ReadDir {
        /// The directory that was read.
        path: PathBuf,
        /// Why it failed.
        source: SysError,
    },
}

/// The explainable outcome of probing one device node.
#[derive(Debug)]
pub enum ProbeVerdict {
    /// The device qualifies as a touchpad candidate; `descriptor` is what
    /// the decoder is configured with.
    Candidate {
        /// Platform-neutral device description for the decoder.
        descriptor: DeviceDescriptor,
    },
    /// The device was probed but does not qualify; each string is one failed
    /// requirement (empty when every requirement passed but another rule
    /// applied — see the module docs).
    Rejected {
        /// Human-readable rejection reasons, one per failed check.
        reasons: Vec<String>,
    },
    /// The device could not be probed at all; the error is actionable.
    Inaccessible {
        /// Why probing failed (permission, ioctl failure, ...).
        error: String,
    },
}

/// Capability bit arrays read from one device, with query helpers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceCapabilities {
    /// `evbit` (`EV_*` codes present).
    pub ev_bits: Vec<u8>,
    /// `keybit` (`KEY_*`/`BTN_*` codes present).
    pub key_bits: Vec<u8>,
    /// `absbit` (`ABS_*` codes present).
    pub abs_bits: Vec<u8>,
    /// `propbit` (`INPUT_PROP_*` properties set).
    pub prop_bits: Vec<u8>,
}

impl DeviceCapabilities {
    /// Whether the device reports event type `code` in its `evbit`.
    #[must_use]
    pub fn has_ev(&self, code: u16) -> bool {
        test_bit(&self.ev_bits, code)
    }

    /// Whether the device reports key/button `code` in its `keybit`.
    #[must_use]
    pub fn has_key(&self, code: u16) -> bool {
        test_bit(&self.key_bits, code)
    }

    /// Whether the device reports absolute axis `code` in its `absbit`.
    #[must_use]
    pub fn has_abs(&self, code: u16) -> bool {
        test_bit(&self.abs_bits, code)
    }

    /// Whether input property `prop` is set.
    #[must_use]
    pub fn has_prop(&self, prop: u16) -> bool {
        test_bit(&self.prop_bits, prop)
    }

    /// Whether the device reports any of `BTN_LEFT`/`BTN_RIGHT`/`BTN_MIDDLE`
    /// (as `EV_KEY` events).
    #[must_use]
    pub fn has_physical_buttons(&self) -> bool {
        self.has_ev(EV_KEY)
            && (self.has_key(BTN_LEFT) || self.has_key(BTN_RIGHT) || self.has_key(BTN_MIDDLE))
    }

    /// Whether all four Type-B MT axes are present (`ABS_MT_SLOT`,
    /// `ABS_MT_TRACKING_ID`, `ABS_MT_POSITION_X/Y`).
    #[must_use]
    pub fn is_type_b(&self) -> bool {
        self.has_abs(ABS_MT_SLOT)
            && self.has_abs(ABS_MT_TRACKING_ID)
            && self.has_abs(ABS_MT_POSITION_X)
            && self.has_abs(ABS_MT_POSITION_Y)
    }

    /// Whether the device is an indirect pointer or buttonpad
    /// (`INPUT_PROP_POINTER` or `INPUT_PROP_BUTTONPAD`).
    #[must_use]
    pub fn is_pointer_like(&self) -> bool {
        self.has_prop(INPUT_PROP_POINTER) || self.has_prop(INPUT_PROP_BUTTONPAD)
    }

    /// Whether the device is a direct-touch device (`INPUT_PROP_DIRECT`,
    /// e.g. a touchscreen).
    #[must_use]
    pub fn is_direct(&self) -> bool {
        self.has_prop(INPUT_PROP_DIRECT)
    }
}

/// Everything learned about one device node.
#[derive(Debug)]
pub struct ProbeReport {
    /// The probed device node.
    pub path: PathBuf,
    /// Device name as reported by `EVIOCGNAME` (empty when unknown).
    pub name: String,
    /// Device identity from `EVIOCGID` (zeros when unknown).
    pub id: InputId,
    /// The explainable outcome.
    pub verdict: ProbeVerdict,
    /// Ordered positive observations used to justify the verdict.
    pub evidence: Vec<String>,
    /// Raw `EVIOCGABS` results for every reported axis (keyed by ABS code).
    pub axes: BTreeMap<u16, AbsInfo>,
    /// Number of Type-B slots derived from `ABS_MT_SLOT.max + 1`, when the
    /// axis was reported.
    pub slot_count: Option<u32>,
    /// Capabilities read from the device.
    pub capabilities: DeviceCapabilities,
}

impl ProbeReport {
    /// The candidate descriptor, when the verdict is [`ProbeVerdict::Candidate`].
    #[must_use]
    pub fn candidate_descriptor(&self) -> Option<&DeviceDescriptor> {
        match &self.verdict {
            ProbeVerdict::Candidate { descriptor } => Some(descriptor),
            _ => None,
        }
    }
}

/// Whether `path` looks like a `/dev/input/event*` node (`event` followed by
/// one or more ASCII digits).
#[must_use]
pub fn is_event_node(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(rest) = name.strip_prefix("event") else {
        return false;
    };
    !rest.is_empty() && rest.bytes().all(|byte| byte.is_ascii_digit())
}

/// Enumerates `/dev/input`, probes every `event*` node, and returns one
/// [`ProbeReport`] per node (sorted by path for determinism). Only a failure
/// to read the directory itself is an error.
pub fn enumerate(sys: &dyn Sys) -> Result<Vec<ProbeReport>, ProbeError> {
    let dir = Path::new("/dev/input");
    let mut entries = sys.read_dir(dir).map_err(|source| ProbeError::ReadDir {
        path: dir.to_path_buf(),
        source,
    })?;
    entries.sort();
    let mut reports = Vec::new();
    for path in entries {
        if is_event_node(&path) {
            reports.push(probe(sys, &path));
        }
    }
    Ok(reports)
}

/// Deterministically picks the first candidate from `reports` (index into
/// `reports`), or `None` when no device qualifies.
#[must_use]
pub fn pick_candidate(reports: &[ProbeReport]) -> Option<usize> {
    reports
        .iter()
        .position(|report| matches!(report.verdict, ProbeVerdict::Candidate { .. }))
}

/// Everything learned from probing an already-open device fd.
#[derive(Debug)]
pub(crate) struct OpenedProbeData {
    /// Device name as reported by `EVIOCGNAME`.
    pub name: String,
    /// Device identity from `EVIOCGID`.
    pub id: InputId,
    /// Capability bit arrays read from the device.
    pub capabilities: DeviceCapabilities,
    /// Raw `EVIOCGABS` results for every reported axis (keyed by ABS code).
    pub axes: BTreeMap<u16, AbsInfo>,
    /// Number of Type-B slots derived from `ABS_MT_SLOT.max + 1`, when the
    /// axis was reported.
    pub slot_count: Option<u32>,
}

/// Reads all capability/axis/slot information from an **already-open** device
/// fd (M4 review R4).
///
/// Shared by the enumeration probe ([`probe`], which opens its own temporary
/// handle) and the runtime session open ([`crate::runtime`], which validates
/// the exact fd it will read from), so the capability/axis/slot rules cannot
/// drift between the two paths.
///
/// Every required capability response is validated for completeness (M4
/// review R7): a successful `EVIOCGBIT`/`EVIOCGPROP` with a full-size buffer
/// copies the whole kernel bit array, so a shorter response is a
/// truncated/mock-corrupt device and fails as
/// [`SysError::TruncatedResponse`] instead of being silently treated as
/// complete.
pub(crate) fn probe_open_fd(sys: &dyn Sys, fd: Fd) -> Result<OpenedProbeData, SysError> {
    let name = read_device_name(sys, fd)?;
    let id = sys.ioctl_id(fd)?;

    let mut ev_bits = vec![0u8; bits_to_bytes(EV_MAX)];
    let returned = sys.ioctl_ev_bits(fd, 0, &mut ev_bits)?;
    require_complete("EVIOCGBIT(0, evbit)", returned, ev_bits.len())?;
    let mut key_bits = vec![0u8; bits_to_bytes(KEY_MAX)];
    let returned = sys.ioctl_ev_bits(fd, EV_KEY, &mut key_bits)?;
    require_complete("EVIOCGBIT(EV_KEY, keybit)", returned, key_bits.len())?;
    let mut abs_bits = vec![0u8; bits_to_bytes(ABS_MAX)];
    let returned = sys.ioctl_ev_bits(fd, EV_ABS, &mut abs_bits)?;
    require_complete("EVIOCGBIT(EV_ABS, absbit)", returned, abs_bits.len())?;
    let mut prop_bits = vec![0u8; bits_to_bytes(INPUT_PROP_MAX)];
    let returned = sys.ioctl_prop_bits(fd, &mut prop_bits)?;
    require_complete("EVIOCGPROP(propbit)", returned, prop_bits.len())?;

    let mut axes: BTreeMap<u16, AbsInfo> = BTreeMap::new();
    for code in 0..=ABS_MAX {
        if test_bit(&abs_bits, code) {
            axes.insert(code, sys.ioctl_absinfo(fd, code)?);
        }
    }

    let capabilities = DeviceCapabilities {
        ev_bits,
        key_bits,
        abs_bits,
        prop_bits,
    };
    let slot_count = axes
        .get(&ABS_MT_SLOT)
        .map(|info| info.max.saturating_add(1).max(0) as u32);

    Ok(OpenedProbeData {
        name,
        id,
        capabilities,
        axes,
        slot_count,
    })
}

/// Fails closed when an ioctl response does not cover the data the caller
/// consumes (M4 review R7). Oversized responses are accepted (they are
/// forward-compatible and bounded by the caller's fixed-size buffers).
fn require_complete(
    operation: &'static str,
    returned: usize,
    required: usize,
) -> Result<(), SysError> {
    if returned < required {
        return Err(SysError::TruncatedResponse {
            operation,
            returned,
            required,
        });
    }
    Ok(())
}

/// Probes one device node and returns an explainable report (never fails —
/// an unopenable or unprobeable node becomes [`ProbeVerdict::Inaccessible`]).
pub fn probe(sys: &dyn Sys, path: &Path) -> ProbeReport {
    let mut evidence = vec![format!(
        "path {} matches the /dev/input/event* enumeration pattern",
        path.display()
    )];
    let axes: BTreeMap<u16, AbsInfo> = BTreeMap::new();
    let slot_count: Option<u32> = None;

    let fd = match sys.open(path) {
        Ok(fd) => fd,
        Err(error) => {
            return ProbeReport {
                path: path.to_path_buf(),
                name: String::new(),
                id: InputId::default(),
                verdict: ProbeVerdict::Inaccessible {
                    error: error.to_string(),
                },
                evidence,
                axes,
                slot_count,
                capabilities: DeviceCapabilities::default(),
            };
        }
    };

    // The probe handle is always closed before returning.
    let data = match probe_open_fd(sys, fd) {
        Ok(data) => data,
        Err(error) => return finish_inaccessible(sys, fd, path, evidence, error),
    };
    let capabilities = data.capabilities;
    let axes = data.axes;
    let slot_count = data.slot_count;

    evidence.extend(describe_capabilities(&capabilities, &axes));

    let verdict = match decide_verdict(
        &data.name,
        data.id,
        &capabilities,
        &axes,
        slot_count,
        &mut evidence,
    ) {
        Ok(descriptor) => ProbeVerdict::Candidate { descriptor },
        Err(reasons) => ProbeVerdict::Rejected { reasons },
    };
    close_probe_handle(sys, fd, &mut evidence);

    ProbeReport {
        path: path.to_path_buf(),
        name: data.name,
        id: data.id,
        verdict,
        evidence,
        axes,
        slot_count,
        capabilities,
    }
}

/// Builds the positive evidence strings for a probed device.
fn describe_capabilities(
    capabilities: &DeviceCapabilities,
    axes: &BTreeMap<u16, AbsInfo>,
) -> Vec<String> {
    let mut evidence = Vec::new();
    if capabilities.has_ev(EV_KEY) {
        evidence.push("reports EV_KEY".to_string());
    }
    if capabilities.has_ev(EV_ABS) {
        evidence.push("reports EV_ABS".to_string());
    }
    if capabilities.is_type_b() {
        evidence.push(
            "Type-B multitouch: ABS_MT_SLOT, ABS_MT_TRACKING_ID, ABS_MT_POSITION_X and ABS_MT_POSITION_Y all present"
                .to_string(),
        );
    }
    if capabilities.has_prop(INPUT_PROP_POINTER) {
        evidence.push("INPUT_PROP_POINTER set (indirect pointer device)".to_string());
    }
    if capabilities.has_prop(INPUT_PROP_BUTTONPAD) {
        evidence.push("INPUT_PROP_BUTTONPAD set (unified buttonpad)".to_string());
    }
    if capabilities.has_prop(INPUT_PROP_DIRECT) {
        evidence.push("INPUT_PROP_DIRECT set (direct-touch device)".to_string());
    }
    if let Some(info) = axes.get(&ABS_MT_SLOT) {
        evidence.push(format!(
            "ABS_MT_SLOT range [0, {}] implies slot_count = {}",
            info.max,
            info.max.saturating_add(1).max(0)
        ));
    }
    let buttons: Vec<&str> = [
        (BTN_LEFT, "BTN_LEFT"),
        (BTN_RIGHT, "BTN_RIGHT"),
        (BTN_MIDDLE, "BTN_MIDDLE"),
    ]
    .iter()
    .filter(|(code, _)| capabilities.has_key(*code))
    .map(|(_, label)| *label)
    .collect();
    if buttons.is_empty() {
        evidence.push("no physical buttons (BTN_LEFT/RIGHT/MIDDLE) reported".to_string());
    } else {
        evidence.push(format!("physical buttons: {}", buttons.join(", ")));
    }
    evidence
}

/// Applies the candidate rule and produces the explainable verdict.
///
/// Returns the candidate [`DeviceDescriptor`] on success, or the rejection
/// reasons (one per failed check) on failure. Shared by the enumeration
/// probe and the runtime session open so both paths apply exactly the same
/// candidate rule to the same fd (M4 review R4).
pub(crate) fn decide_verdict(
    name: &str,
    id: InputId,
    capabilities: &DeviceCapabilities,
    axes: &BTreeMap<u16, AbsInfo>,
    slot_count: Option<u32>,
    evidence: &mut Vec<String>,
) -> Result<DeviceDescriptor, Vec<String>> {
    let mut reasons: Vec<String> = Vec::new();

    if !capabilities.is_type_b() {
        let missing: Vec<String> = [
            (ABS_MT_SLOT, "ABS_MT_SLOT"),
            (ABS_MT_TRACKING_ID, "ABS_MT_TRACKING_ID"),
            (ABS_MT_POSITION_X, "ABS_MT_POSITION_X"),
            (ABS_MT_POSITION_Y, "ABS_MT_POSITION_Y"),
        ]
        .iter()
        .filter(|(code, _)| !capabilities.has_abs(*code))
        .map(|(_, label)| (*label).to_string())
        .collect();
        reasons.push(format!(
            "not Type-B multitouch: missing {} (the decoder requires ABS_MT_SLOT, ABS_MT_TRACKING_ID, ABS_MT_POSITION_X and ABS_MT_POSITION_Y)",
            missing.join(", ")
        ));
    }

    if capabilities.is_direct() {
        reasons.push(
            "INPUT_PROP_DIRECT set: a direct-touch device (e.g. touchscreen), not an indirect touchpad"
                .to_string(),
        );
    }
    if !capabilities.is_pointer_like() {
        reasons.push(
            "neither INPUT_PROP_POINTER nor INPUT_PROP_BUTTONPAD set: not an indirect pointer/buttonpad device"
                .to_string(),
        );
    }

    if let Some(count) = slot_count {
        if count == 0 || count > MAX_SLOT_COUNT {
            reasons.push(format!(
                "slot count {count} is outside the supported range [1, {MAX_SLOT_COUNT}]"
            ));
        }
    }

    if reasons.is_empty() {
        let mut descriptor = DeviceDescriptor::new(name.to_string(), id.vendor, id.product);
        descriptor.axes = axes
            .iter()
            .map(|(code, info)| (crate::codes::axis_id_for_code(*code), to_axis_info(*info)))
            .collect();
        descriptor.slot_count = slot_count;
        descriptor.supports_type_b_mt = true;
        descriptor.has_physical_buttons = capabilities.has_physical_buttons();
        descriptor.profile = DeviceProfile::for_hardware_named(name, id.vendor, id.product);
        evidence.push(format!(
            "accepted: Type-B multitouch pointer device with {} slots",
            slot_count.unwrap_or_default()
        ));
        Ok(descriptor)
    } else {
        evidence.push(format!("rejected for {} reason(s)", reasons.len()));
        Err(reasons)
    }
}

/// Converts a raw kernel `input_absinfo` into the core [`AxisInfo`], mapping
/// a non-positive resolution to "not reported".
fn to_axis_info(info: AbsInfo) -> AxisInfo {
    AxisInfo::new(
        info.min,
        info.max,
        info.fuzz,
        info.flat,
        NonZeroU32::new(info.resolution.max(0) as u32),
    )
}

/// Reads the NUL-terminated device name via `EVIOCGNAME`.
fn read_device_name(sys: &dyn Sys, fd: Fd) -> Result<String, SysError> {
    let mut buf = [0u8; 256];
    let n = sys.ioctl_name(fd, &mut buf)?;
    let n = n.min(buf.len());
    let end = buf[..n].iter().position(|byte| *byte == 0).unwrap_or(n);
    Ok(String::from_utf8_lossy(&buf[..end]).into_owned())
}

/// Closes the probe handle, noting any close failure in `evidence`.
fn close_probe_handle(sys: &dyn Sys, fd: Fd, evidence: &mut Vec<String>) {
    if let Err(error) = sys.close(fd) {
        evidence.push(format!("warning: closing the probe handle failed: {error}"));
    }
}

/// Returns an [`ProbeVerdict::Inaccessible`] report after closing the probe
/// handle.
fn finish_inaccessible(
    sys: &dyn Sys,
    fd: Fd,
    path: &Path,
    mut evidence: Vec<String>,
    error: SysError,
) -> ProbeReport {
    close_probe_handle(sys, fd, &mut evidence);
    ProbeReport {
        path: path.to_path_buf(),
        name: String::new(),
        id: InputId::default(),
        verdict: ProbeVerdict::Inaccessible {
            error: error.to_string(),
        },
        evidence,
        axes: BTreeMap::new(),
        slot_count: None,
        capabilities: DeviceCapabilities::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use crate::codes::{ABS_MT_PRESSURE, BTN_LEFT, INPUT_PROP_SEMI_MT};
    use crate::sys::mock::{MockDevice, MockFailure, MockSys};

    fn probe_path(sys: &MockSys, name: &str) -> ProbeReport {
        let path = PathBuf::from(format!("/dev/input/{name}"));
        sys.add_device(&path, MockDevice::touchpad("Test Touchpad", 12));
        probe(sys, &path)
    }

    #[test]
    fn candidate_touchpad_is_accepted_with_explainable_evidence() {
        let sys = MockSys::new();
        let report = probe_path(&sys, "event0");
        let descriptor = match &report.verdict {
            ProbeVerdict::Candidate { descriptor } => descriptor,
            other => panic!("expected candidate, got {other:?}"),
        };
        assert_eq!(descriptor.name, "Test Touchpad");
        assert_eq!(descriptor.slot_count, Some(12));
        assert!(descriptor.supports_type_b_mt);
        assert!(report
            .evidence
            .iter()
            .any(|line| line.contains("Type-B multitouch")));
        assert!(report
            .evidence
            .iter()
            .any(|line| line.contains("INPUT_PROP_POINTER")));
        // The four required MT axes must be present in the descriptor.
        for code in [
            ABS_MT_SLOT,
            ABS_MT_TRACKING_ID,
            ABS_MT_POSITION_X,
            ABS_MT_POSITION_Y,
        ] {
            assert!(
                descriptor
                    .axes
                    .contains_key(&crate::codes::axis_id_for_code(code)),
                "descriptor must carry ABS code {code}"
            );
        }
    }

    #[test]
    fn non_type_b_device_is_rejected_with_a_reason() {
        let sys = MockSys::new();
        let path = PathBuf::from("/dev/input/event0");
        let mut device = MockDevice::touchpad("Missing Tracking", 8);
        // Remove ABS_MT_TRACKING_ID from the absbit and absinfo.
        device.abs_bits[crate::ABS_MT_TRACKING_ID as usize / 8] &=
            !(1 << (crate::ABS_MT_TRACKING_ID % 8));
        device.absinfo.remove(&crate::ABS_MT_TRACKING_ID);
        sys.add_device(&path, device);
        let report = probe(&sys, &path);
        match &report.verdict {
            ProbeVerdict::Rejected { reasons } => {
                assert!(
                    reasons.iter().any(|r| r.contains("not Type-B multitouch")),
                    "{reasons:?}"
                );
            }
            other => panic!("expected rejection, got {other:?}"),
        }
    }

    #[test]
    fn direct_touch_device_is_rejected() {
        let sys = MockSys::new();
        let path = PathBuf::from("/dev/input/event0");
        let mut device = MockDevice::touchpad("Touchscreen", 10);
        device.add_prop(crate::INPUT_PROP_DIRECT);
        device.prop_bits[crate::INPUT_PROP_POINTER as usize / 8] &=
            !(1 << (crate::INPUT_PROP_POINTER % 8));
        sys.add_device(&path, device);
        let report = probe(&sys, &path);
        match &report.verdict {
            ProbeVerdict::Rejected { reasons } => {
                assert!(
                    reasons.iter().any(|r| r.contains("INPUT_PROP_DIRECT")),
                    "{reasons:?}"
                );
            }
            other => panic!("expected rejection, got {other:?}"),
        }
    }

    #[test]
    fn non_pointer_non_buttonpad_device_is_rejected() {
        let sys = MockSys::new();
        let path = PathBuf::from("/dev/input/event0");
        let mut device = MockDevice::touchpad("No Props", 10);
        device.prop_bits = vec![0; bits_to_bytes(INPUT_PROP_MAX)];
        sys.add_device(&path, device);
        let report = probe(&sys, &path);
        match &report.verdict {
            ProbeVerdict::Rejected { reasons } => {
                assert!(
                    reasons.iter().any(|r| r.contains("INPUT_PROP_POINTER")),
                    "{reasons:?}"
                );
            }
            other => panic!("expected rejection, got {other:?}"),
        }
    }

    #[test]
    fn oversized_slot_count_is_rejected() {
        let sys = MockSys::new();
        let path = PathBuf::from("/dev/input/event0");
        let mut device = MockDevice::touchpad("Too Many Slots", 10);
        // Claim 1000 slots via the ABS_MT_SLOT absinfo maximum.
        device.absinfo.insert(
            crate::ABS_MT_SLOT,
            AbsInfo {
                value: 0,
                min: 0,
                max: 999,
                fuzz: 0,
                flat: 0,
                resolution: 0,
            },
        );
        sys.add_device(&path, device);
        let report = probe(&sys, &path);
        match &report.verdict {
            ProbeVerdict::Rejected { reasons } => {
                assert!(
                    reasons.iter().any(|r| r.contains("slot count 1000")),
                    "{reasons:?}"
                );
            }
            other => panic!("expected rejection, got {other:?}"),
        }
    }

    #[test]
    fn permission_denied_open_is_inaccessible() {
        let sys = MockSys::new();
        let path = PathBuf::from("/dev/input/event0");
        sys.set_open_error(&path, MockFailure::PermissionDenied);
        let report = probe(&sys, &path);
        match &report.verdict {
            ProbeVerdict::Inaccessible { error } => {
                assert!(error.contains("permission"), "{error}");
            }
            other => panic!("expected inaccessible, got {other:?}"),
        }
    }

    #[test]
    fn ioctl_failure_during_probe_is_inaccessible() {
        let sys = MockSys::new();
        let path = PathBuf::from("/dev/input/event0");
        let mut device = MockDevice::touchpad("Broken", 10);
        device.ioctl_error = Some(MockFailure::Io);
        sys.add_device(&path, device);
        let report = probe(&sys, &path);
        assert!(
            matches!(report.verdict, ProbeVerdict::Inaccessible { .. }),
            "{:?}",
            report.verdict
        );
    }

    /// M4 review R7: a truncated required capability response must not be
    /// silently treated as complete — the probe fails closed with an
    /// actionable `Inaccessible` verdict.
    #[test]
    fn truncated_ev_bits_response_is_inaccessible() {
        let sys = MockSys::new();
        let path = PathBuf::from("/dev/input/event0");
        let mut device = MockDevice::touchpad("Truncated evbit", 10);
        // 2 bytes instead of the full 4-byte evbit array.
        device.ev_bits = vec![0u8; 2];
        sys.add_device(&path, device);
        let report = probe(&sys, &path);
        match &report.verdict {
            ProbeVerdict::Inaccessible { error } => {
                assert!(error.contains("truncated"), "{error}");
                assert!(error.contains("EVIOCGBIT(0, evbit)"), "{error}");
            }
            other => panic!("expected inaccessible, got {other:?}"),
        }
    }

    #[test]
    fn truncated_key_bits_response_is_inaccessible() {
        let sys = MockSys::new();
        let path = PathBuf::from("/dev/input/event0");
        let mut device = MockDevice::touchpad("Truncated keybit", 10);
        device.key_bits = vec![0u8; 20];
        sys.add_device(&path, device);
        let report = probe(&sys, &path);
        match &report.verdict {
            ProbeVerdict::Inaccessible { error } => {
                assert!(error.contains("EVIOCGBIT(EV_KEY, keybit)"), "{error}");
            }
            other => panic!("expected inaccessible, got {other:?}"),
        }
    }

    #[test]
    fn truncated_prop_bits_response_is_inaccessible() {
        let sys = MockSys::new();
        let path = PathBuf::from("/dev/input/event0");
        let mut device = MockDevice::touchpad("Truncated propbit", 10);
        device.prop_bits = vec![0u8; 1];
        sys.add_device(&path, device);
        let report = probe(&sys, &path);
        assert!(
            matches!(report.verdict, ProbeVerdict::Inaccessible { .. }),
            "{:?}",
            report.verdict
        );
    }

    #[test]
    fn physical_buttons_are_reflected_in_the_descriptor() {
        let sys = MockSys::new();
        let path = PathBuf::from("/dev/input/event0");
        let mut device = MockDevice::touchpad("Buttoned", 10);
        device.add_key(BTN_LEFT);
        device.add_key(crate::BTN_RIGHT);
        sys.add_device(&path, device);
        let report = probe(&sys, &path);
        let descriptor = report.candidate_descriptor().expect("candidate");
        assert!(descriptor.has_physical_buttons);
        assert!(report
            .evidence
            .iter()
            .any(|line| line.contains("BTN_LEFT") && line.contains("BTN_RIGHT")));
    }

    #[test]
    fn buttonpad_property_qualifies_a_candidate() {
        let sys = MockSys::new();
        let path = PathBuf::from("/dev/input/event0");
        let mut device = MockDevice::touchpad("Buttonpad", 10);
        device.prop_bits[crate::INPUT_PROP_POINTER as usize / 8] &=
            !(1 << (crate::INPUT_PROP_POINTER % 8));
        device.add_prop(INPUT_PROP_BUTTONPAD);
        sys.add_device(&path, device);
        let report = probe(&sys, &path);
        assert!(
            matches!(report.verdict, ProbeVerdict::Candidate { .. }),
            "{:?}",
            report.verdict
        );
        assert!(report
            .evidence
            .iter()
            .any(|line| line.contains("INPUT_PROP_BUTTONPAD")));
    }

    #[test]
    fn enumerate_filters_event_nodes_and_sorts() {
        let sys = MockSys::new();
        sys.set_dir_entries(vec![
            PathBuf::from("/dev/input/event1"),
            PathBuf::from("/dev/input/mouse0"),
            PathBuf::from("/dev/input/event0"),
            PathBuf::from("/dev/input/event"), // no digits: not a node
            PathBuf::from("/dev/input/event2x"), // trailing non-digit
        ]);
        sys.add_device(
            PathBuf::from("/dev/input/event0"),
            MockDevice::touchpad("a", 4),
        );
        sys.add_device(
            PathBuf::from("/dev/input/event1"),
            MockDevice::touchpad("b", 4),
        );
        let reports = enumerate(&sys).unwrap();
        let paths: Vec<PathBuf> = reports.iter().map(|r| r.path.clone()).collect();
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/dev/input/event0"),
                PathBuf::from("/dev/input/event1")
            ]
        );
        assert_eq!(pick_candidate(&reports), Some(0));
    }

    #[test]
    fn enumerate_missing_input_dir_is_an_error() {
        let sys = MockSys::new();
        sys.set_read_dir_error(MockFailure::NotFound);
        let err = enumerate(&sys).unwrap_err();
        assert!(matches!(err, ProbeError::ReadDir { .. }));
        assert!(err.to_string().contains("/dev/input"), "{err}");
    }

    #[test]
    fn is_event_node_matches_only_digit_suffixed_names() {
        assert!(is_event_node(Path::new("/dev/input/event0")));
        assert!(is_event_node(Path::new("/dev/input/event42")));
        assert!(!is_event_node(Path::new("/dev/input/event")));
        assert!(!is_event_node(Path::new("/dev/input/eventx")));
        assert!(!is_event_node(Path::new("/dev/input/mouse0")));
    }

    #[test]
    fn missing_resolution_maps_to_none_in_axis_info() {
        let info = to_axis_info(AbsInfo {
            value: 0,
            min: 0,
            max: 100,
            fuzz: 0,
            flat: 0,
            resolution: 0,
        });
        assert_eq!(info.resolution, None);
        let info = to_axis_info(AbsInfo {
            value: 0,
            min: 0,
            max: 100,
            fuzz: 0,
            flat: 0,
            resolution: 100,
        });
        assert_eq!(info.resolution, NonZeroU32::new(100));
    }

    #[test]
    fn probe_closes_its_handle() {
        let sys = MockSys::new();
        let path = PathBuf::from("/dev/input/event0");
        sys.add_device(&path, MockDevice::touchpad("pad", 4));
        let _ = probe(&sys, &path);
        // The probe opens and closes exactly once; no handle leaks open.
        assert_eq!(
            sys.count(|call| matches!(call, crate::sys::mock::MockCall::Open(_))),
            1
        );
        assert_eq!(
            sys.count(|call| matches!(call, crate::sys::mock::MockCall::Close(_))),
            1
        );
    }

    #[test]
    fn semi_mt_and_pressure_axes_do_not_disqualify() {
        let sys = MockSys::new();
        let path = PathBuf::from("/dev/input/event0");
        let mut device = MockDevice::touchpad("Semi MT", 10);
        device.add_prop(INPUT_PROP_SEMI_MT);
        device.add_abs(
            ABS_MT_PRESSURE,
            AbsInfo {
                value: 0,
                min: 0,
                max: 255,
                fuzz: 0,
                flat: 0,
                resolution: 0,
            },
        );
        sys.add_device(&path, device);
        let report = probe(&sys, &path);
        assert!(
            matches!(report.verdict, ProbeVerdict::Candidate { .. }),
            "{:?}",
            report.verdict
        );
    }
}
