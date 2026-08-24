//! Type-B multitouch slot decoder and frame commit (M3).
//!
//! The decoder consumes [`RawEvent`]s (live kernel events or replayed trace
//! events) and publishes a [`ContactFrame`] exactly once per `SYN_REPORT`.
//! It implements the Type-B slot protocol (IMPLEMENTATION_BRIEF §5):
//!
//! * `ABS_MT_SLOT` only switches the current slot. An out-of-range selection
//!   **fails closed**: slot selection is revoked and every slot-scoped
//!   `ABS_MT_*` event is ignored (with a diagnostic) until a valid
//!   `ABS_MT_SLOT` arrives — they are never redirected to the previous slot.
//! * `ABS_MT_TRACKING_ID >= 0` begins a contact (a different id on an active
//!   slot is a replacement lifecycle); exactly `ABS_MT_TRACKING_ID == -1`
//!   ends it; any value `< -1` is diagnosed and ignored. Tracking-id
//!   transitions are applied **incrementally at event arrival time** to a
//!   bounded per-slot lifecycle state (end→begin, begin→end, repeated ids,
//!   and replacement chains have deterministic semantics; see
//!   `PendingLifecycle`).
//! * Other `ABS_MT_*` events update the current slot's pending fields **for
//!   the lifecycle that is effective at event arrival time**. A field
//!   arriving while no lifecycle is live (before any begin, or after an
//!   end) is diagnosed and ignored — it can neither complete a prior
//!   contact nor leak into a later one.
//!   `ABS_MT_TOUCH_MAJOR`/`ABS_MT_TOUCH_MINOR` are contact *lengths* and are
//!   normalized with the core delta conversion, never the absolute-position
//!   conversion (which would wrongly subtract the axis origin).
//! * Physical button events join the same pending frame.
//! * Only `SYN_REPORT` merges pending state, increments the frame sequence,
//!   and publishes a frame; a single event never publishes a half frame.
//!
//! New contacts are held until both X and Y coordinates have been reported
//! (IMPLEMENTATION_BRIEF §4): until then they are internal-only and the frame
//! carries a [`DiagnosticCode::IncompleteNewContact`] warning; once complete
//! they are published with a [`DiagnosticCode::DelayedNewContact`] notice.
//! Un-updated fields of an existing contact inherit the previous committed
//! state, but a new tracking lifecycle never inherits a prior contact's
//! fields.
//!
//! ## Synchronization states
//!
//! [`SyncState`] is one of `Normal`, `DroppedAwaitingBoundary`, `Recovering`,
//! or `Degraded` (IMPLEMENTATION_BRIEF §6):
//!
//! 1. `SYN_DROPPED` moves the decoder to `DroppedAwaitingBoundary`; all
//!    incremental events are ignored until the next `SYN_REPORT`.
//! 2. That `SYN_REPORT` moves it to `Recovering` while a [`ResyncSource`]
//!    snapshot is queried (a transient state inside one `feed` call).
//! 3. A **valid** snapshot is validated and normalized into a complete draft
//!    state, which is swapped in atomically only after it succeeds. The
//!    decoder then publishes a complete `discontinuity = true` frame
//!    ([`DiagnosticCode::DecodeRecovered`]) and returns to `Normal`. An
//!    invalid snapshot (out-of-range or duplicate slot, invalid tracking id,
//!    or an active contact missing raw X/Y) is a resync failure.
//! 4. A failed snapshot (query error or invalid content) moves the decoder to
//!    `Degraded` and returns a fatal [`DecodeError::ResyncFailed`]; no
//!    discontinuity frame is published and a degraded decoder never emits a
//!    trusted frame again.
#![forbid(unsafe_code)]

use std::collections::HashSet;

use touchpad_core::{
    raw_axis_delta_to_mm, raw_axis_position_to_mm_with_resolution, AxisConversionError, AxisInfo,
    Contact, ContactFrame, ContactState, DeviceDescriptor, Diagnostic, DiagnosticCode,
    DiagnosticLevel, Millimeters, Monotonic, PhysicalButtons, RawAxis,
};

use crate::codes::{
    axis_id_for_code, ABS_MT_ORIENTATION, ABS_MT_POSITION_X, ABS_MT_POSITION_Y, ABS_MT_PRESSURE,
    ABS_MT_SLOT, ABS_MT_TOUCH_MAJOR, ABS_MT_TOUCH_MINOR, ABS_MT_TRACKING_ID, BTN_LEFT, BTN_MIDDLE,
    BTN_RIGHT, EV_ABS, EV_KEY, EV_SYN, SYN_DROPPED, SYN_REPORT,
};
use crate::rawevent::RawEvent;
use crate::resync::{KernelStateSnapshot, ResyncSource, SlotSnapshot};
use crate::sink::FrameSink;

/// The maximum Type-B slot count the decoder accepts from a device
/// descriptor.
///
/// The Linux input subsystem has no hard global cap, but no known Type-B
/// touchpad reports more than a few dozen slots. `256` is a generous safety
/// ceiling that keeps the decoder's per-slot state bounded and prevents an
/// untrusted (e.g. replay-controlled) header from requesting an effectively
/// unbounded allocation. A descriptor with a larger `slot_count` is rejected
/// with [`DecodeError::InvalidDevice`] before any decoder state is built.
pub const MAX_SLOT_COUNT: u32 = 256;

/// Decoder synchronization state (IMPLEMENTATION_BRIEF §5/§6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncState {
    /// Normal decoding: every event is processed and every `SYN_REPORT`
    /// commits a frame.
    Normal,
    /// `SYN_DROPPED` was received; incremental events are ignored until the
    /// next `SYN_REPORT` boundary.
    DroppedAwaitingBoundary,
    /// The `SYN_REPORT` boundary after a drop was reached; a kernel state
    /// snapshot is being queried. Transient — entered and left within one
    /// `feed` call.
    Recovering,
    /// Resynchronization failed (or the decoder is otherwise unusable); the
    /// decoder is terminal and never emits a trusted frame again.
    Degraded,
}

/// Failure modes of the Type-B decoder.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum DecodeError {
    /// [`TypeBDecoder::feed`] was called before [`TypeBDecoder::configure`].
    #[error("decoder is not configured with a device descriptor")]
    NotConfigured,
    /// [`TypeBDecoder::configure`] was called more than once.
    #[error("decoder is already configured with a device descriptor")]
    AlreadyConfigured,
    /// The descriptor cannot drive Type-B decoding.
    #[error("device descriptor is not usable for Type-B decoding: {0}")]
    InvalidDevice(String),
    /// `SYN_DROPPED` recovery failed; the decoder is degraded.
    #[error("input stream lost continuity and resynchronization failed; decoder is degraded: {0}")]
    ResyncFailed(String),
    /// A feed was attempted after the decoder degraded.
    #[error(
        "decoder is degraded after a failed resynchronization; it will never emit trusted frames again"
    )]
    Degraded,
}

/// Committed per-slot state: the last published kernel frame, plus the
/// bookkeeping the decoder needs for the incomplete-new-contact policy.
#[derive(Clone, Debug)]
struct SlotState {
    tracking_id: i32,
    x_mm: Option<Millimeters>,
    y_mm: Option<Millimeters>,
    pressure: Option<f32>,
    major_mm: Option<Millimeters>,
    minor_mm: Option<Millimeters>,
    orientation: Option<f32>,
    /// Whether an X position was ever reported for this tracking id.
    x_known: bool,
    /// Whether a Y position was ever reported for this tracking id.
    y_known: bool,
    /// Whether the current tracking id was published at least once.
    published: bool,
    /// Whether the incomplete-new-contact warning was already emitted for the
    /// current tracking id (emitted once per lifecycle).
    incomplete_diagnosed: bool,
}

impl SlotState {
    fn empty() -> Self {
        Self {
            tracking_id: -1,
            x_mm: None,
            y_mm: None,
            pressure: None,
            major_mm: None,
            minor_mm: None,
            orientation: None,
            x_known: false,
            y_known: false,
            published: false,
            incomplete_diagnosed: false,
        }
    }

    fn reset(&mut self) {
        *self = Self::empty();
    }

    fn is_complete(&self) -> bool {
        self.x_known && self.y_known
    }
}

/// The **effective lifecycle** of one slot at a given moment within a
/// `SYN_REPORT` cycle.
///
/// The decoder processes `ABS_MT_TRACKING_ID` events incrementally, at event
/// arrival time, so that every field update is associated with the lifecycle
/// that was active when the field arrived (M3 review R2, re-review 1). This
/// enum is the bounded per-slot lifecycle state machine: there is no
/// transition list, so a replay-controlled stream cannot grow per-slot
/// memory without a `SYN_REPORT` boundary.
///
/// A cycle always starts from the committed state: a slot holding a contact
/// begins the cycle `Active { id, fresh: false }`, an empty slot begins
/// `Empty`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum PendingLifecycle {
    /// A tracking id is live. `fresh` is `true` when the lifecycle began
    /// within this cycle (it must not inherit the previous contact's
    /// fields); `false` for a lifecycle that crossed the frame boundary.
    Active { id: i32, fresh: bool },
    /// No lifecycle is live and none has been ended this cycle.
    #[default]
    Empty,
    /// A lifecycle was ended this cycle (`ABS_MT_TRACKING_ID == -1`); no
    /// lifecycle is live. Fields arriving in this state are diagnosed and
    /// ignored — they must not alter the ended contact or leak into a later
    /// begin.
    Ended,
}

/// How many tracking-id replacement steps the decoder retains for
/// diagnostics per slot per cycle.
///
/// Replacement chains are already pathological at a few steps; retaining the
/// first `MAX_TRACKING_REPLACEMENTS` steps and summarizing the rest keeps the
/// per-slot pending state constant-memory (a replay-controlled stream cannot
/// grow it without a `SYN_REPORT` boundary).
const MAX_TRACKING_REPLACEMENTS: usize = 16;

/// Tracking-id replacement steps `(old, new)` for diagnostics, bounded to
/// [`MAX_TRACKING_REPLACEMENTS`] entries plus an overflow counter.
#[derive(Clone, Debug, Default)]
struct Replacements {
    steps: Vec<(i32, i32)>,
    overflow: u32,
}

impl Replacements {
    /// Records one replacement step `old -> new`.
    fn record(&mut self, old: i32, new: i32) {
        if self.steps.len() < MAX_TRACKING_REPLACEMENTS {
            self.steps.push((old, new));
        } else {
            self.overflow = self.overflow.saturating_add(1);
        }
    }
}

/// Pending per-slot updates for the current `SYN_REPORT` cycle.
///
/// All state here is bounded: the effective lifecycle plus at most one field
/// value per axis and a fixed-size replacement-diagnostic buffer. Fields are
/// owned by the lifecycle that was active when they arrived and are cleared
/// whenever a tracking transition starts a new lifecycle, so no field from a
/// previous lifecycle can leak into a newer one (M3 review R2, re-review 1).
#[derive(Clone, Debug, Default)]
struct PendingSlot {
    /// The slot's effective lifecycle, updated incrementally at event
    /// arrival time.
    lifecycle: PendingLifecycle,
    /// Field updates belonging to the **current** effective lifecycle only.
    /// Cleared on every lifecycle transition that starts a new lifecycle.
    x: Option<RawAxis>,
    y: Option<RawAxis>,
    pressure: Option<RawAxis>,
    touch_major: Option<RawAxis>,
    touch_minor: Option<RawAxis>,
    orientation: Option<RawAxis>,
    /// The committed (pre-existing) contact was ended by an explicit
    /// `end(-1)` this cycle: an `Ended` contact is published at the boundary
    /// when no new lifecycle is live then.
    ended_committed: bool,
    /// A lifecycle began **and** ended entirely within this cycle; it never
    /// crossed a frame boundary and is not published.
    began_then_ended: bool,
    /// An `end(-1)` arrived while no lifecycle was live; diagnosed at the
    /// boundary.
    end_without_contact: bool,
    /// Replacement steps observed this cycle, for diagnostics.
    replacements: Replacements,
}

impl PendingSlot {
    /// Discards the field bucket. Called when a tracking transition starts a
    /// new lifecycle, so the previous lifecycle's fields never leak into it.
    fn clear_fields(&mut self) {
        self.x = None;
        self.y = None;
        self.pressure = None;
        self.touch_major = None;
        self.touch_minor = None;
        self.orientation = None;
    }
}

/// Normalized pending field values for one slot in one frame.
#[derive(Clone, Copy, Debug)]
struct NormalizedFields {
    x_mm: Option<Millimeters>,
    y_mm: Option<Millimeters>,
    pressure: Option<f32>,
    major_mm: Option<Millimeters>,
    minor_mm: Option<Millimeters>,
    orientation: Option<f32>,
}

/// The Type-B multitouch slot decoder.
///
/// Construct with a [`FrameSink`], configure with a [`DeviceDescriptor`]
/// (live input does this once at startup; the [`crate::replay`] path does it
/// from the trace header, so replay uses the same device model as live
/// input), then call [`TypeBDecoder::feed`] for every raw event. Committed
/// frames are published to the sink in order.
pub struct TypeBDecoder<S: FrameSink> {
    device: Option<DeviceDescriptor>,
    slot_count: u32,
    current_slot: u32,
    /// Whether the current slot selection is valid. Initially `true` (the
    /// protocol default selects slot 0); an out-of-range `ABS_MT_SLOT`
    /// revokes it and all slot-scoped events are ignored until a valid
    /// selection arrives (fail-closed, M3 review R1).
    slot_selection_valid: bool,
    sync_state: SyncState,
    /// Whether the most recent [`TypeBDecoder::feed`] call performed a
    /// successful resynchronization (snapshot installed + discontinuity
    /// frame published). Reset to `false` at the start of every `feed`;
    /// consumed by the runtime to drain the rest of a read batch whose
    /// events predate the installed snapshot (M4 review R6).
    just_resynced: bool,
    committed: Vec<SlotState>,
    pending: Vec<PendingSlot>,
    buttons: PhysicalButtons,
    sequence: u64,
    frame_diagnostics: Vec<Diagnostic>,
    resync: Option<Box<dyn ResyncSource>>,
    sink: S,
}

impl<S: FrameSink> TypeBDecoder<S> {
    /// Creates an unconfigured decoder over `sink`.
    #[must_use]
    pub fn new(sink: S) -> Self {
        Self {
            device: None,
            slot_count: 0,
            current_slot: 0,
            slot_selection_valid: true,
            sync_state: SyncState::Normal,
            just_resynced: false,
            committed: Vec::new(),
            pending: Vec::new(),
            buttons: PhysicalButtons::NONE,
            sequence: 0,
            frame_diagnostics: Vec::new(),
            resync: None,
            sink,
        }
    }

    /// Sets the [`ResyncSource`] used for `SYN_DROPPED` recovery.
    ///
    /// Live input (M4) will install the real kernel snapshot adapter here;
    /// tests and offline replay install a mock.
    pub fn set_resync_source(&mut self, source: Box<dyn ResyncSource>) {
        self.resync = Some(source);
    }

    /// Builder-style variant of [`TypeBDecoder::set_resync_source`].
    #[must_use]
    pub fn with_resync_source(mut self, source: Box<dyn ResyncSource>) -> Self {
        self.resync = Some(source);
        self
    }

    /// The current synchronization state.
    #[must_use]
    pub fn sync_state(&self) -> SyncState {
        self.sync_state
    }

    /// Whether the most recent [`TypeBDecoder::feed`] call performed a
    /// successful resync (installed a kernel snapshot and published a
    /// discontinuity frame).
    ///
    /// The live runtime uses this to drain the remainder of the current read
    /// batch: those events were queued before the snapshot ioctl observed
    /// kernel state, so replaying them would apply pre-snapshot deltas on
    /// top of the newer snapshot (M4 review R6).
    #[must_use]
    pub fn just_resynced(&self) -> bool {
        self.just_resynced
    }

    /// Consumes the decoder and returns its frame sink.
    #[must_use]
    pub fn into_sink(self) -> S {
        self.sink
    }

    /// A mutable reference to the frame sink (M10: the takeover coordinator
    /// prepares a streaming output session through the decoder's sink after
    /// the device is open but before any read or grab).
    #[must_use]
    pub fn sink_mut(&mut self) -> &mut S {
        &mut self.sink
    }

    /// Configures the decoder with the device's descriptor.
    ///
    /// Must be called exactly once, before the first [`TypeBDecoder::feed`].
    /// Live input passes the probed descriptor; replay passes the trace
    /// header's descriptor.
    pub fn configure(&mut self, device: DeviceDescriptor) -> Result<(), DecodeError> {
        if self.device.is_some() {
            return Err(DecodeError::AlreadyConfigured);
        }
        if !device.supports_type_b_mt {
            return Err(DecodeError::InvalidDevice(
                "descriptor does not report Type-B multitouch support".to_string(),
            ));
        }
        let slot_count = device.slot_count.ok_or_else(|| {
            DecodeError::InvalidDevice("descriptor reports no Type-B slot count".to_string())
        })?;
        if slot_count == 0 {
            return Err(DecodeError::InvalidDevice(
                "descriptor reports zero Type-B slots".to_string(),
            ));
        }
        if slot_count > MAX_SLOT_COUNT {
            return Err(DecodeError::InvalidDevice(format!(
                "descriptor reports slot_count {slot_count}, which exceeds the decoder's documented maximum of {MAX_SLOT_COUNT} supported Type-B slots"
            )));
        }
        let errors = device.validate();
        if let Some(error) = errors
            .iter()
            .find(|diagnostic| diagnostic.level == DiagnosticLevel::Error)
        {
            return Err(DecodeError::InvalidDevice(format!(
                "descriptor validation failed: {}",
                error.message
            )));
        }
        self.device = Some(device);
        self.slot_count = slot_count;
        self.committed = vec![SlotState::empty(); slot_count as usize];
        self.pending = vec![PendingSlot::default(); slot_count as usize];
        Ok(())
    }

    /// Feeds one raw input event into the decoder.
    ///
    /// This is the single entry point shared by live input and replay: live
    /// input (M4) builds [`RawEvent`]s from kernel `input_event` structs and
    /// replay converts [`touchpad_trace::TraceEvent`]s. A frame is published (to the sink)
    /// only at a `SYN_REPORT`; a single event never publishes a half frame.
    ///
    /// Returns an error when the decoder is degraded after a failed
    /// resynchronization ([`DecodeError::Degraded`]) or when recovery itself
    /// failed ([`DecodeError::ResyncFailed`]); after that the decoder never
    /// emits a trusted frame again.
    pub fn feed(&mut self, event: RawEvent) -> Result<(), DecodeError> {
        if self.sync_state == SyncState::Degraded {
            return Err(DecodeError::Degraded);
        }
        if self.device.is_none() {
            return Err(DecodeError::NotConfigured);
        }
        self.just_resynced = false;
        match event.event_type {
            EV_SYN => match event.code {
                SYN_REPORT => self.on_syn_report(event.timestamp)?,
                SYN_DROPPED => {
                    self.sync_state = SyncState::DroppedAwaitingBoundary;
                }
                _ => {}
            },
            // Incremental ABS/KEY events are processed only in Normal: while
            // DroppedAwaitingBoundary they are ignored until the next
            // SYN_REPORT (Recovering never spans two feed calls; Degraded is
            // rejected at the entry guard). Everything else is ignored.
            EV_ABS if self.sync_state == SyncState::Normal => self.on_abs(event),
            EV_KEY if self.sync_state == SyncState::Normal => self.on_key(event),
            _ => {}
        }
        Ok(())
    }

    fn on_syn_report(&mut self, timestamp: Monotonic) -> Result<(), DecodeError> {
        match self.sync_state {
            SyncState::Normal => {
                self.commit(timestamp);
                Ok(())
            }
            SyncState::DroppedAwaitingBoundary => self.resync(timestamp),
            SyncState::Recovering | SyncState::Degraded => {
                // Defensive: Recovering never spans two feed calls and
                // Degraded is rejected at the feed entry; ignore rather than
                // panic.
                Ok(())
            }
        }
    }

    fn on_abs(&mut self, event: RawEvent) {
        match event.code {
            ABS_MT_SLOT => {
                if event.value < 0 || event.value as u32 >= self.slot_count {
                    // Fail closed: the selection is revoked, and every
                    // slot-scoped event is ignored until a valid slot is
                    // selected. They are never redirected to the previous
                    // slot (M3 review R1).
                    self.slot_selection_valid = false;
                    self.frame_diagnostics.push(Diagnostic::new(
                        DiagnosticLevel::Error,
                        DiagnosticCode::SlotOutOfRange,
                        format!(
                            "ABS_MT_SLOT value {} is outside the device's slot range [0, {})",
                            event.value, self.slot_count
                        ),
                    ));
                } else {
                    self.slot_selection_valid = true;
                    self.current_slot = event.value as u32;
                }
            }
            ABS_MT_TRACKING_ID | ABS_MT_POSITION_X | ABS_MT_POSITION_Y | ABS_MT_PRESSURE
            | ABS_MT_TOUCH_MAJOR | ABS_MT_TOUCH_MINOR | ABS_MT_ORIENTATION => {
                if !self.slot_selection_valid {
                    self.frame_diagnostics.push(Diagnostic::new(
                        DiagnosticLevel::Warning,
                        DiagnosticCode::InvalidEventOrder,
                        format!(
                            "slot-scoped ABS_MT event (code {}) ignored: the last ABS_MT_SLOT selection was invalid, so no slot is selected until a valid ABS_MT_SLOT arrives",
                            event.code
                        ),
                    ));
                    return;
                }
                let slot = self.current_slot as usize;
                match event.code {
                    ABS_MT_TRACKING_ID => {
                        // Exactly -1 ends a contact; values < -1 are invalid
                        // and are diagnosed and ignored (M3 review R2). Valid
                        // transitions are applied to the slot's effective
                        // lifecycle immediately, at arrival time, so every
                        // later field update is associated with the lifecycle
                        // that is live when it arrives (M3 review R2,
                        // re-review 1).
                        if event.value < -1 {
                            self.frame_diagnostics.push(Diagnostic::new(
                                DiagnosticLevel::Warning,
                                DiagnosticCode::InvalidEventOrder,
                                format!(
                                    "ABS_MT_TRACKING_ID value {} is not valid: only ids >= 0 begin a contact and exactly -1 ends one; the event is ignored",
                                    event.value
                                ),
                            ));
                        } else if event.value == -1 {
                            let pending = &mut self.pending[slot];
                            match pending.lifecycle {
                                PendingLifecycle::Active { fresh, .. } => {
                                    if fresh {
                                        // This lifecycle began and ended
                                        // inside the cycle: it never crosses
                                        // a frame boundary and its fields are
                                        // irrelevant.
                                        pending.began_then_ended = true;
                                        pending.clear_fields();
                                    } else {
                                        // The committed contact ended; its
                                        // pre-end fields are kept so the
                                        // Ended contact carries its final
                                        // position.
                                        pending.ended_committed = true;
                                    }
                                    pending.lifecycle = PendingLifecycle::Ended;
                                }
                                PendingLifecycle::Empty | PendingLifecycle::Ended => {
                                    pending.end_without_contact = true;
                                }
                            }
                        } else {
                            let pending = &mut self.pending[slot];
                            match pending.lifecycle {
                                PendingLifecycle::Active { id: current, .. }
                                    if current == event.value =>
                                {
                                    // Repeated begin of the same effective id:
                                    // a no-op that must not reset the
                                    // lifecycle or its fields (M3 review R2).
                                }
                                PendingLifecycle::Active { id: current, .. } => {
                                    // A real replacement: start a clean
                                    // lifecycle and discard the previous
                                    // lifecycle's fields (M3 review R2,
                                    // re-review 1).
                                    pending.replacements.record(current, event.value);
                                    pending.lifecycle = PendingLifecycle::Active {
                                        id: event.value,
                                        fresh: true,
                                    };
                                    pending.clear_fields();
                                }
                                PendingLifecycle::Empty | PendingLifecycle::Ended => {
                                    // A new lifecycle begins; fields recorded
                                    // while no lifecycle was live are never
                                    // kept (they are diagnosed and ignored),
                                    // so the bucket starts clean.
                                    pending.lifecycle = PendingLifecycle::Active {
                                        id: event.value,
                                        fresh: true,
                                    };
                                    pending.clear_fields();
                                }
                            }
                        }
                    }
                    ABS_MT_POSITION_X | ABS_MT_POSITION_Y | ABS_MT_PRESSURE
                    | ABS_MT_TOUCH_MAJOR | ABS_MT_TOUCH_MINOR | ABS_MT_ORIENTATION => {
                        let lifecycle = self.pending[slot].lifecycle;
                        match lifecycle {
                            PendingLifecycle::Active { .. } => {
                                let value = RawAxis::new(event.value);
                                let pending = &mut self.pending[slot];
                                match event.code {
                                    ABS_MT_POSITION_X => pending.x = Some(value),
                                    ABS_MT_POSITION_Y => pending.y = Some(value),
                                    ABS_MT_PRESSURE => pending.pressure = Some(value),
                                    ABS_MT_TOUCH_MAJOR => pending.touch_major = Some(value),
                                    ABS_MT_TOUCH_MINOR => pending.touch_minor = Some(value),
                                    ABS_MT_ORIENTATION => pending.orientation = Some(value),
                                    _ => {}
                                }
                            }
                            PendingLifecycle::Empty => {
                                // No lifecycle is (or ever was) live: the
                                // field is diagnosed and ignored, never
                                // recorded for a later begin (M3 review R2,
                                // re-review 1).
                                self.frame_diagnostics.push(Diagnostic::new(
                                    DiagnosticLevel::Warning,
                                    DiagnosticCode::InvalidEventOrder,
                                    format!(
                                        "ABS_MT field event (code {}) arrived for slot {slot} with no active tracking id; it is ignored",
                                        event.code
                                    ),
                                ));
                            }
                            PendingLifecycle::Ended => {
                                // The contact already ended this cycle: the
                                // field must not alter the prior Ended
                                // contact, so it is diagnosed and ignored
                                // (M3 review R2, re-review 1).
                                self.frame_diagnostics.push(Diagnostic::new(
                                    DiagnosticLevel::Warning,
                                    DiagnosticCode::InvalidEventOrder,
                                    format!(
                                        "ABS_MT field event (code {}) arrived for slot {slot} after its contact ended; it is ignored",
                                        event.code
                                    ),
                                ));
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn on_key(&mut self, event: RawEvent) {
        match event.code {
            BTN_LEFT => self.buttons.left = event.value != 0,
            BTN_RIGHT => self.buttons.right = event.value != 0,
            BTN_MIDDLE => self.buttons.middle = event.value != 0,
            // Other key codes (BTN_TOUCH, BTN_TOOL_*) are not physical
            // buttons and are ignored.
            _ => {}
        }
    }

    /// Resynchronizes after `SYN_DROPPED` at the next `SYN_REPORT` boundary.
    ///
    /// Enters `Recovering`, queries the [`ResyncSource`], then either
    /// atomically replaces the decoder state and publishes a complete
    /// `discontinuity = true` frame (back to `Normal`), or enters `Degraded`
    /// and returns a fatal error. An invalid or incomplete snapshot is a
    /// resync failure: `Degraded` is entered and **no** discontinuity frame
    /// is published (M3 review R4).
    fn resync(&mut self, timestamp: Monotonic) -> Result<(), DecodeError> {
        self.sync_state = SyncState::Recovering;
        let snapshot = match self.resync.as_mut() {
            Some(source) => match source.snapshot() {
                Ok(snapshot) => snapshot,
                Err(err) => {
                    self.sync_state = SyncState::Degraded;
                    return Err(DecodeError::ResyncFailed(err.to_string()));
                }
            },
            None => {
                self.sync_state = SyncState::Degraded;
                return Err(DecodeError::ResyncFailed(
                    "no resync source is configured; a kernel state snapshot is required"
                        .to_string(),
                ));
            }
        };
        if let Err(reason) = self.apply_snapshot(&snapshot) {
            self.sync_state = SyncState::Degraded;
            return Err(DecodeError::ResyncFailed(format!(
                "invalid resync snapshot: {reason}"
            )));
        }
        self.sync_state = SyncState::Normal;
        self.just_resynced = true;
        self.publish_discontinuity_frame(timestamp);
        Ok(())
    }

    /// Validates a kernel snapshot completely, builds a complete draft state,
    /// and swaps it in only after validation/normalization succeeds.
    ///
    /// Returns `Err(reason)` — without touching any live decoder state — for
    /// an out-of-range or duplicate slot, an invalid tracking id (`< -1`), or
    /// an active contact missing its required raw X or Y coordinate. The
    /// caller must treat that as a resync failure and enter `Degraded`.
    fn apply_snapshot(&mut self, snapshot: &KernelStateSnapshot) -> Result<(), String> {
        let slot_count = self.slot_count as usize;

        // Phase 1: validate the snapshot completely, before any state change.
        let mut seen_slots = HashSet::new();
        for slot in &snapshot.slots {
            if slot.slot as usize >= slot_count {
                return Err(format!(
                    "snapshot reports slot {} outside the device's slot range [0, {})",
                    slot.slot, self.slot_count
                ));
            }
            if !seen_slots.insert(slot.slot) {
                return Err(format!("snapshot lists slot {} more than once", slot.slot));
            }
            if slot.tracking_id < -1 {
                return Err(format!(
                    "snapshot reports invalid tracking id {} for slot {} (only >= 0 or exactly -1 are valid)",
                    slot.tracking_id, slot.slot
                ));
            }
            if slot.tracking_id >= 0 && (slot.position_x.is_none() || slot.position_y.is_none()) {
                return Err(format!(
                    "snapshot reports active slot {} (tracking id {}) without required raw X/Y coordinates",
                    slot.slot, slot.tracking_id
                ));
            }
        }

        // Phase 2: build the complete draft state (normalization may record
        // missing-resolution diagnostics but never fails the resync).
        let mut new_committed: Vec<SlotState> =
            (0..slot_count).map(|_| SlotState::empty()).collect();
        let mut new_missing: Vec<u16> = Vec::new();
        for slot in &snapshot.slots {
            if slot.tracking_id < 0 {
                continue;
            }
            let index = slot.slot as usize;
            let normalized = self.normalize_snapshot_slot(slot, &mut new_missing);
            let committed = &mut new_committed[index];
            committed.tracking_id = slot.tracking_id;
            if slot.position_x.is_some() {
                committed.x_known = true;
                committed.x_mm = normalized.x_mm;
            }
            if slot.position_y.is_some() {
                committed.y_known = true;
                committed.y_mm = normalized.y_mm;
            }
            if slot.pressure.is_some() {
                committed.pressure = normalized.pressure;
            }
            if slot.touch_major.is_some() {
                committed.major_mm = normalized.major_mm;
            }
            if slot.touch_minor.is_some() {
                committed.minor_mm = normalized.minor_mm;
            }
            if slot.orientation.is_some() {
                committed.orientation = normalized.orientation;
            }
            // The consumer learns about these contacts in the discontinuity
            // frame, so the next frame marks them Active, not Began again.
            committed.published = true;
        }
        // The draft pending state starts each slot's cycle from the draft
        // committed state: a resynced live contact is an ongoing (non-fresh)
        // lifecycle, so the next cycle's fields attach to it.
        let new_pending: Vec<PendingSlot> = (0..slot_count)
            .map(|slot| {
                let mut pending = PendingSlot::default();
                let id = new_committed[slot].tracking_id;
                if id >= 0 {
                    pending.lifecycle = PendingLifecycle::Active { id, fresh: false };
                }
                pending
            })
            .collect();

        // Phase 3: atomic swap, only after validation and construction
        // succeeded.
        self.committed = new_committed;
        self.pending = new_pending;
        self.buttons = snapshot.physical_buttons;
        self.current_slot = 0;
        self.slot_selection_valid = true;
        self.frame_diagnostics.clear();
        for code in new_missing {
            self.frame_diagnostics
                .push(missing_resolution_diagnostic(code));
        }
        Ok(())
    }

    /// Publishes the complete `discontinuity = true` frame after a successful
    /// resynchronization. Every active snapshot slot appears as a fresh
    /// `Began` contact, since consumers cannot compare this frame with the
    /// previous one.
    fn publish_discontinuity_frame(&mut self, timestamp: Monotonic) {
        self.sequence += 1;
        let sequence = self.sequence;
        let mut contacts = Vec::new();
        for (slot, state) in self.committed.iter().enumerate() {
            if state.tracking_id >= 0 {
                contacts.push(contact_from(slot as u32, state, ContactState::Began));
            }
        }
        self.frame_diagnostics.push(Diagnostic::new(
            DiagnosticLevel::Info,
            DiagnosticCode::DecodeRecovered,
            format!(
                "input stream lost continuity (SYN_DROPPED) and was resynchronized at frame {sequence}"
            ),
        ));
        let mut diagnostics = std::mem::take(&mut self.frame_diagnostics);
        stamp_diagnostics(&mut diagnostics, sequence);
        let frame = ContactFrame {
            monotonic_timestamp: timestamp,
            sequence,
            discontinuity: true,
            contacts,
            physical_buttons: self.buttons,
            diagnostics,
        };
        self.sink.on_frame(frame);
    }

    /// Merges pending state into committed state, increments the frame
    /// sequence, and publishes exactly one frame. Called only at `SYN_REPORT`
    /// while in [`SyncState::Normal`].
    fn commit(&mut self, timestamp: Monotonic) {
        self.sequence += 1;
        let sequence = self.sequence;
        let mut diagnostics = std::mem::take(&mut self.frame_diagnostics);
        let mut contacts: Vec<Contact> = Vec::new();
        let mut missing: Vec<u16> = Vec::new();

        for slot in 0..self.slot_count as usize {
            let pending = std::mem::take(&mut self.pending[slot]);
            let normalized = self.normalize_pending_slot(&pending, &mut missing);
            let mut slot_diagnostics: Vec<Diagnostic> = Vec::new();

            // The effective lifecycle at the boundary was maintained
            // incrementally at event arrival time (M3 review R2, re-review
            // 1): `end(-1) -> begin(new)` leaves the new lifecycle live here,
            // `begin(new) -> end(-1)` leaves none, and every field bucket
            // already belongs to exactly one lifecycle.
            let committed = &mut self.committed[slot];
            match pending.lifecycle {
                PendingLifecycle::Active { id, fresh } => {
                    if fresh {
                        // A lifecycle live at the boundary that began this
                        // cycle: fields from the prior contact must never
                        // leak into it.
                        for (old, new_id) in &pending.replacements.steps {
                            slot_diagnostics
                                .push(tracking_replaced_diagnostic(slot, *old, *new_id));
                        }
                        push_replacement_overflow_diagnostic(
                            &mut slot_diagnostics,
                            slot,
                            &pending.replacements,
                        );
                        committed.reset();
                        committed.tracking_id = id;
                    }
                    apply_pending_fields(committed, &pending, normalized);
                }
                PendingLifecycle::Empty | PendingLifecycle::Ended => {
                    // No lifecycle is live at the boundary.
                    if pending.ended_committed {
                        // The committed contact was ended this cycle; its
                        // pre-end fields (if any) describe its final state.
                        apply_pending_fields(committed, &pending, normalized);
                        if committed.published {
                            contacts.push(contact_from(
                                slot as u32,
                                committed,
                                ContactState::Ended,
                            ));
                        }
                        committed.reset();
                    }
                    if pending.end_without_contact {
                        slot_diagnostics.push(Diagnostic::new(
                            DiagnosticLevel::Warning,
                            DiagnosticCode::InvalidEventOrder,
                            format!(
                                "slot {slot} reported a tracking-id end with no active contact"
                            ),
                        ));
                    }
                    if pending.began_then_ended {
                        for (old, new_id) in &pending.replacements.steps {
                            slot_diagnostics
                                .push(tracking_replaced_diagnostic(slot, *old, *new_id));
                        }
                        push_replacement_overflow_diagnostic(
                            &mut slot_diagnostics,
                            slot,
                            &pending.replacements,
                        );
                        slot_diagnostics.push(Diagnostic::new(
                            DiagnosticLevel::Warning,
                            DiagnosticCode::InvalidEventOrder,
                            format!(
                                "slot {slot} contact began and ended within a single frame; it is not published"
                            ),
                        ));
                        committed.reset();
                    }
                }
            }

            if committed.tracking_id >= 0 {
                if !committed.is_complete() {
                    if !committed.incomplete_diagnosed {
                        slot_diagnostics.push(Diagnostic::new(
                            DiagnosticLevel::Warning,
                            DiagnosticCode::IncompleteNewContact,
                            format!(
                                "new contact on slot {slot} (tracking id {}) has incomplete coordinates; held until both X and Y are reported",
                                committed.tracking_id
                            ),
                        ));
                        committed.incomplete_diagnosed = true;
                    }
                } else if !committed.published {
                    if committed.incomplete_diagnosed {
                        slot_diagnostics.push(Diagnostic::new(
                            DiagnosticLevel::Info,
                            DiagnosticCode::DelayedNewContact,
                            format!(
                                "contact on slot {slot} (tracking id {}) is published after its coordinates became complete",
                                committed.tracking_id
                            ),
                        ));
                        committed.incomplete_diagnosed = false;
                    }
                    committed.published = true;
                    contacts.push(contact_from(slot as u32, committed, ContactState::Began));
                } else {
                    contacts.push(contact_from(slot as u32, committed, ContactState::Active));
                }
            }

            // Start the next cycle from the state we just committed: a live
            // contact is an ongoing (non-fresh) lifecycle, an empty slot an
            // empty lifecycle. Keeping this in the pending state means the
            // very next field event of the next cycle attaches to the right
            // lifecycle without replaying any history.
            let next_id = committed.tracking_id;
            self.pending[slot].lifecycle = if next_id >= 0 {
                PendingLifecycle::Active {
                    id: next_id,
                    fresh: false,
                }
            } else {
                PendingLifecycle::Empty
            };

            diagnostics.extend(slot_diagnostics);
        }

        for code in missing {
            diagnostics.push(missing_resolution_diagnostic(code));
        }
        stamp_diagnostics(&mut diagnostics, sequence);

        let frame = ContactFrame {
            monotonic_timestamp: timestamp,
            sequence,
            discontinuity: false,
            contacts,
            physical_buttons: self.buttons,
            diagnostics,
        };
        self.sink.on_frame(frame);
    }

    /// Normalizes the pending raw values of one slot for the current frame.
    fn normalize_pending_slot(
        &self,
        pending: &PendingSlot,
        missing: &mut Vec<u16>,
    ) -> NormalizedFields {
        NormalizedFields {
            x_mm: normalize_pending_position(self, ABS_MT_POSITION_X, pending.x, missing),
            y_mm: normalize_pending_position(self, ABS_MT_POSITION_Y, pending.y, missing),
            major_mm: normalize_pending_length(
                self,
                ABS_MT_TOUCH_MAJOR,
                pending.touch_major,
                missing,
            ),
            minor_mm: normalize_pending_length(
                self,
                ABS_MT_TOUCH_MINOR,
                pending.touch_minor,
                missing,
            ),
            pressure: pending
                .pressure
                .and_then(|raw| self.normalize_pressure(raw)),
            orientation: pending
                .orientation
                .and_then(|raw| self.normalize_orientation(raw)),
        }
    }

    /// Normalizes the raw values of one snapshot slot for the discontinuity
    /// frame.
    fn normalize_snapshot_slot(
        &self,
        slot: &SlotSnapshot,
        missing: &mut Vec<u16>,
    ) -> NormalizedFields {
        NormalizedFields {
            x_mm: normalize_pending_position(self, ABS_MT_POSITION_X, slot.position_x, missing),
            y_mm: normalize_pending_position(self, ABS_MT_POSITION_Y, slot.position_y, missing),
            major_mm: normalize_pending_length(self, ABS_MT_TOUCH_MAJOR, slot.touch_major, missing),
            minor_mm: normalize_pending_length(self, ABS_MT_TOUCH_MINOR, slot.touch_minor, missing),
            pressure: slot.pressure.and_then(|raw| self.normalize_pressure(raw)),
            orientation: slot
                .orientation
                .and_then(|raw| self.normalize_orientation(raw)),
        }
    }

    /// Converts a raw absolute position to millimeters using the axis the
    /// Linux layer maps this ABS code to, honoring the device resolution or
    /// an explicit profile override. Fails with
    /// [`AxisConversionError::MissingResolution`] when neither is available —
    /// the value then stays unnormalized with a diagnostic, never a fake
    /// millimeter.
    fn normalize_position(
        &self,
        code: u16,
        raw: RawAxis,
    ) -> Result<Millimeters, AxisConversionError> {
        let device = self
            .device
            .as_ref()
            .ok_or(AxisConversionError::MissingResolution)?;
        let axis = axis_id_for_code(code);
        let info = device
            .axes
            .get(&axis)
            .ok_or(AxisConversionError::MissingResolution)?;
        let resolution = device
            .profile
            .effective_resolution(axis, info)
            .ok_or(AxisConversionError::MissingResolution)?;
        raw_axis_position_to_mm_with_resolution(raw, info, resolution)
    }

    /// Converts a raw contact **length/delta** (touch major/minor) to
    /// millimeters using the axis's resolution or an explicit profile
    /// override.
    ///
    /// Lengths have no axis origin: the conversion is `raw / resolution`
    /// (core [`raw_axis_delta_to_mm`]), deliberately *not* the absolute
    /// position conversion, which would wrongly subtract `AxisInfo::min` and
    /// shift the physical contact size (M3 review R3).
    fn normalize_length(
        &self,
        code: u16,
        raw: RawAxis,
    ) -> Result<Millimeters, AxisConversionError> {
        let device = self
            .device
            .as_ref()
            .ok_or(AxisConversionError::MissingResolution)?;
        let axis = axis_id_for_code(code);
        let info = device
            .axes
            .get(&axis)
            .ok_or(AxisConversionError::MissingResolution)?;
        let resolution = device
            .profile
            .effective_resolution(axis, info)
            .ok_or(AxisConversionError::MissingResolution)?;
        raw_axis_delta_to_mm(raw, Some(resolution))
    }

    /// Normalizes raw pressure to `[0, 1]` over the declared axis range.
    /// `None` when the device does not declare a usable pressure axis.
    fn normalize_pressure(&self, raw: RawAxis) -> Option<f32> {
        let device = self.device.as_ref()?;
        let info = device.axes.get(&axis_id_for_code(ABS_MT_PRESSURE))?;
        let t = normalized_unit(raw, info)?;
        Some(t.clamp(0.0, 1.0))
    }

    /// Normalizes raw orientation to radians by mapping the full declared raw
    /// range to `[-PI/2, +PI/2]` (the kernel "signed quarter of a revolution"
    /// convention). Devices with a different orientation convention are
    /// handled by `DeviceQuirk` in later milestones. `None` when the device
    /// does not declare a usable orientation axis.
    fn normalize_orientation(&self, raw: RawAxis) -> Option<f32> {
        let device = self.device.as_ref()?;
        let info = device.axes.get(&axis_id_for_code(ABS_MT_ORIENTATION))?;
        let t = normalized_unit(raw, info)?;
        Some((t - 0.5) * std::f32::consts::PI)
    }
}

/// Normalizes one raw absolute position, recording the ABS code in `missing`
/// (deduplicated) when no resolution is available.
fn normalize_pending_position(
    decoder: &TypeBDecoder<impl FrameSink>,
    code: u16,
    raw: Option<RawAxis>,
    missing: &mut Vec<u16>,
) -> Option<Millimeters> {
    let raw = raw?;
    match decoder.normalize_position(code, raw) {
        Ok(mm) => Some(mm),
        Err(AxisConversionError::MissingResolution) => {
            if !missing.contains(&code) {
                missing.push(code);
            }
            None
        }
        Err(AxisConversionError::NonFinite) => None,
    }
}

/// Normalizes one raw contact length (touch major/minor), recording the ABS
/// code in `missing` (deduplicated) when no resolution is available.
fn normalize_pending_length(
    decoder: &TypeBDecoder<impl FrameSink>,
    code: u16,
    raw: Option<RawAxis>,
    missing: &mut Vec<u16>,
) -> Option<Millimeters> {
    let raw = raw?;
    match decoder.normalize_length(code, raw) {
        Ok(mm) => Some(mm),
        Err(AxisConversionError::MissingResolution) => {
            if !missing.contains(&code) {
                missing.push(code);
            }
            None
        }
        Err(AxisConversionError::NonFinite) => None,
    }
}

/// Maps a raw axis value to `[0, 1]` over the declared range; `None` for a
/// degenerate (empty) range.
fn normalized_unit(raw: RawAxis, info: &AxisInfo) -> Option<f32> {
    let min = i64::from(info.min);
    let max = i64::from(info.max);
    if max <= min {
        return None;
    }
    let t = (i64::from(raw.as_i32()) - min) as f64 / (max - min) as f64;
    Some(t as f32)
}

fn tracking_replaced_diagnostic(slot: usize, old: i32, new_id: i32) -> Diagnostic {
    Diagnostic::new(
        DiagnosticLevel::Info,
        DiagnosticCode::TrackingIdReplaced,
        format!("slot {slot} tracking id replaced {old} -> {new_id}"),
    )
}

/// Pushes a summary diagnostic when more replacement steps occurred in one
/// cycle than [`MAX_TRACKING_REPLACEMENTS`], so the per-slot diagnostics stay
/// bounded while the truncation is still reported.
fn push_replacement_overflow_diagnostic(
    diagnostics: &mut Vec<Diagnostic>,
    slot: usize,
    replacements: &Replacements,
) {
    if replacements.overflow > 0 {
        diagnostics.push(Diagnostic::new(
            DiagnosticLevel::Info,
            DiagnosticCode::TrackingIdReplaced,
            format!(
                "slot {slot} tracking id was replaced {} additional times within one frame; further replacement diagnostics are suppressed after {MAX_TRACKING_REPLACEMENTS}",
                replacements.overflow
            ),
        ));
    }
}

fn missing_resolution_diagnostic(code: u16) -> Diagnostic {
    Diagnostic::new(
        DiagnosticLevel::Warning,
        DiagnosticCode::MissingAxisResolution,
        format!(
            "axis {code} (ABS code {code}) has no reported resolution and no profile override; its value stays unnormalized"
        ),
    )
}

fn stamp_diagnostics(diagnostics: &mut [Diagnostic], sequence: u64) {
    for diagnostic in diagnostics {
        diagnostic.frame_sequence = Some(sequence);
    }
}

/// Applies this frame's normalized field updates onto the committed slot
/// state. Fields the device did not report this frame are left inherited.
fn apply_pending_fields(
    committed: &mut SlotState,
    pending: &PendingSlot,
    normalized: NormalizedFields,
) {
    if pending.x.is_some() {
        committed.x_known = true;
        committed.x_mm = normalized.x_mm;
    }
    if pending.y.is_some() {
        committed.y_known = true;
        committed.y_mm = normalized.y_mm;
    }
    if pending.pressure.is_some() {
        committed.pressure = normalized.pressure;
    }
    if pending.touch_major.is_some() {
        committed.major_mm = normalized.major_mm;
    }
    if pending.touch_minor.is_some() {
        committed.minor_mm = normalized.minor_mm;
    }
    if pending.orientation.is_some() {
        committed.orientation = normalized.orientation;
    }
}

/// Builds a [`Contact`] snapshot from a slot's committed state.
fn contact_from(slot: u32, state: &SlotState, contact_state: ContactState) -> Contact {
    Contact {
        tracking_id: state.tracking_id,
        slot,
        x_mm: state.x_mm,
        y_mm: state.y_mm,
        pressure: state.pressure,
        major_mm: state.major_mm,
        minor_mm: state.minor_mm,
        orientation: state.orientation,
        state: contact_state,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::num::NonZeroU32;
    use std::rc::Rc;

    use touchpad_core::DeviceDescriptor;

    use crate::resync::SlotSnapshot;
    use crate::sink::RecordingFrameSink;

    fn axis(min: i32, max: i32, resolution: Option<NonZeroU32>) -> AxisInfo {
        AxisInfo::new(min, max, 0, 0, resolution)
    }

    fn type_b_descriptor() -> DeviceDescriptor {
        let mut device = DeviceDescriptor::new("test touchpad", 0x1234, 0x5678);
        device.supports_type_b_mt = true;
        device.slot_count = Some(10);
        device.axes.insert(
            axis_id_for_code(ABS_MT_POSITION_X),
            axis(0, 1000, NonZeroU32::new(100)),
        );
        device.axes.insert(
            axis_id_for_code(ABS_MT_POSITION_Y),
            axis(0, 1000, NonZeroU32::new(100)),
        );
        device
    }

    fn descriptor_with_pressure() -> DeviceDescriptor {
        let mut device = type_b_descriptor();
        device.axes.insert(
            axis_id_for_code(ABS_MT_PRESSURE),
            axis(0, 100, NonZeroU32::new(1)),
        );
        device
    }

    fn descriptor_without_resolution() -> DeviceDescriptor {
        let mut device = type_b_descriptor();
        device
            .axes
            .insert(axis_id_for_code(ABS_MT_POSITION_X), axis(0, 1000, None));
        device
            .axes
            .insert(axis_id_for_code(ABS_MT_POSITION_Y), axis(0, 1000, None));
        device
    }

    /// A descriptor whose touch-major/minor axes have a **non-zero minimum**.
    /// Contact lengths must be converted as deltas (`raw / resolution`), so a
    /// raw value of 150 must become 1.5 mm — the absolute-position conversion
    /// (which subtracts the origin) would wrongly produce 0.5 mm (M3 review
    /// R3).
    fn descriptor_with_nonzero_min_lengths() -> DeviceDescriptor {
        let mut device = type_b_descriptor();
        device.axes.insert(
            axis_id_for_code(ABS_MT_TOUCH_MAJOR),
            axis(100, 500, NonZeroU32::new(100)),
        );
        device.axes.insert(
            axis_id_for_code(ABS_MT_TOUCH_MINOR),
            axis(100, 500, NonZeroU32::new(100)),
        );
        device
    }

    fn ev(usec: u64, event_type: u16, code: u16, value: i32) -> RawEvent {
        RawEvent::new(Monotonic::from_nanos(usec * 1000), event_type, code, value)
    }

    fn slot(usec: u64, value: i32) -> RawEvent {
        ev(usec, EV_ABS, ABS_MT_SLOT, value)
    }

    fn tid(usec: u64, value: i32) -> RawEvent {
        ev(usec, EV_ABS, ABS_MT_TRACKING_ID, value)
    }

    fn x(usec: u64, value: i32) -> RawEvent {
        ev(usec, EV_ABS, ABS_MT_POSITION_X, value)
    }

    fn y(usec: u64, value: i32) -> RawEvent {
        ev(usec, EV_ABS, ABS_MT_POSITION_Y, value)
    }

    fn pressure(usec: u64, value: i32) -> RawEvent {
        ev(usec, EV_ABS, ABS_MT_PRESSURE, value)
    }

    fn touch_major(usec: u64, value: i32) -> RawEvent {
        ev(usec, EV_ABS, ABS_MT_TOUCH_MAJOR, value)
    }

    fn touch_minor(usec: u64, value: i32) -> RawEvent {
        ev(usec, EV_ABS, ABS_MT_TOUCH_MINOR, value)
    }

    fn syn(usec: u64) -> RawEvent {
        ev(usec, EV_SYN, SYN_REPORT, 0)
    }

    fn dropped(usec: u64) -> RawEvent {
        ev(usec, EV_SYN, SYN_DROPPED, 0)
    }

    fn btn(usec: u64, code: u16, value: i32) -> RawEvent {
        ev(usec, EV_KEY, code, value)
    }

    fn mm(raw: i32) -> Millimeters {
        let info = axis(0, 1000, NonZeroU32::new(100));
        raw_axis_position_to_mm_with_resolution(
            RawAxis::new(raw),
            &info,
            NonZeroU32::new(100).unwrap(),
        )
        .unwrap()
    }

    /// Expected millimeters for a raw *length* at resolution 100 (delta
    /// conversion: `raw / 100`, no origin subtraction).
    fn length_mm(raw: i32) -> Millimeters {
        raw_axis_delta_to_mm(RawAxis::new(raw), NonZeroU32::new(100)).unwrap()
    }

    fn contact(
        tracking_id: i32,
        slot: u32,
        state: ContactState,
        x_mm: Option<Millimeters>,
        y_mm: Option<Millimeters>,
    ) -> Contact {
        Contact {
            tracking_id,
            slot,
            x_mm,
            y_mm,
            pressure: None,
            major_mm: None,
            minor_mm: None,
            orientation: None,
            state,
        }
    }

    #[derive(Debug, Clone, PartialEq)]
    struct MockError(String);

    impl std::fmt::Display for MockError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.0)
        }
    }

    impl std::error::Error for MockError {}

    enum MockResync {
        Ok(KernelStateSnapshot),
        Err(&'static str),
        Counting(Rc<Cell<usize>>, KernelStateSnapshot),
    }

    impl ResyncSource for MockResync {
        fn snapshot(
            &mut self,
        ) -> Result<KernelStateSnapshot, Box<dyn std::error::Error + Send + Sync>> {
            match self {
                MockResync::Ok(snapshot) => Ok(snapshot.clone()),
                MockResync::Err(message) => Err(Box::new(MockError((*message).to_string()))),
                MockResync::Counting(counter, snapshot) => {
                    counter.set(counter.get() + 1);
                    Ok(snapshot.clone())
                }
            }
        }
    }

    struct Harness {
        decoder: TypeBDecoder<RecordingFrameSink>,
    }

    impl Harness {
        fn new() -> Self {
            Self {
                decoder: TypeBDecoder::new(RecordingFrameSink::new()),
            }
        }

        fn configured(device: DeviceDescriptor) -> Self {
            let mut harness = Self::new();
            harness.decoder.configure(device).unwrap();
            harness
        }

        fn frames(&self) -> &[ContactFrame] {
            self.decoder.sink.frames()
        }
    }

    fn feed_all(decoder: &mut TypeBDecoder<RecordingFrameSink>, events: &[RawEvent]) {
        for event in events {
            decoder.feed(*event).unwrap();
        }
    }

    #[test]
    fn single_contact_begin_update_end() {
        let mut harness = Harness::configured(type_b_descriptor());
        feed_all(
            &mut harness.decoder,
            &[
                slot(1000, 0),
                tid(1000, 10),
                x(1000, 500),
                y(1000, 400),
                syn(1000),
                x(1100, 520),
                y(1100, 405),
                syn(1100),
                tid(1200, -1),
                syn(1200),
            ],
        );
        let frames = harness.frames();
        assert_eq!(frames.len(), 3);
        assert_eq!(
            frames[0],
            ContactFrame {
                monotonic_timestamp: Monotonic::from_nanos(1_000_000),
                sequence: 1,
                discontinuity: false,
                contacts: vec![contact(
                    10,
                    0,
                    ContactState::Began,
                    Some(mm(500)),
                    Some(mm(400))
                )],
                physical_buttons: PhysicalButtons::NONE,
                diagnostics: vec![],
            }
        );
        assert_eq!(
            frames[1].contacts,
            vec![contact(
                10,
                0,
                ContactState::Active,
                Some(mm(520)),
                Some(mm(405))
            )]
        );
        assert_eq!(
            frames[2].contacts,
            vec![contact(
                10,
                0,
                ContactState::Ended,
                Some(mm(520)),
                Some(mm(405))
            )]
        );
        assert_eq!(frames[1].sequence, 2);
        assert_eq!(frames[2].sequence, 3);
        assert_eq!(
            frames[1].monotonic_timestamp,
            Monotonic::from_nanos(1_100_000)
        );
        assert_eq!(
            frames[2].monotonic_timestamp,
            Monotonic::from_nanos(1_200_000)
        );
    }

    #[test]
    fn multiple_slots_and_interleaved_updates() {
        let mut harness = Harness::configured(type_b_descriptor());
        feed_all(
            &mut harness.decoder,
            &[
                slot(1000, 0),
                tid(1000, 1),
                x(1000, 100),
                y(1000, 200),
                slot(1000, 1),
                tid(1000, 2),
                x(1000, 300),
                y(1000, 400),
                syn(1000),
                slot(1100, 0),
                x(1100, 110),
                slot(1100, 1),
                x(1100, 310),
                syn(1100),
                slot(1200, 0),
                tid(1200, -1),
                syn(1200),
                slot(1300, 1),
                tid(1300, -1),
                syn(1300),
            ],
        );
        let frames = harness.frames();
        assert_eq!(frames.len(), 4);
        assert_eq!(
            frames[0].contacts,
            vec![
                contact(1, 0, ContactState::Began, Some(mm(100)), Some(mm(200))),
                contact(2, 1, ContactState::Began, Some(mm(300)), Some(mm(400))),
            ]
        );
        assert_eq!(
            frames[1].contacts,
            vec![
                contact(1, 0, ContactState::Active, Some(mm(110)), Some(mm(200))),
                contact(2, 1, ContactState::Active, Some(mm(310)), Some(mm(400))),
            ]
        );
        assert_eq!(
            frames[2].contacts,
            vec![
                contact(1, 0, ContactState::Ended, Some(mm(110)), Some(mm(200))),
                contact(2, 1, ContactState::Active, Some(mm(310)), Some(mm(400))),
            ]
        );
        assert_eq!(
            frames[3].contacts,
            vec![contact(
                2,
                1,
                ContactState::Ended,
                Some(mm(310)),
                Some(mm(400))
            )]
        );
    }

    #[test]
    fn slot_switch_does_not_alter_other_slots_pending() {
        // Selecting slot 1 and updating it must not disturb slot 0's pending
        // updates in the same frame, and vice versa.
        let mut harness = Harness::configured(type_b_descriptor());
        feed_all(
            &mut harness.decoder,
            &[
                slot(1000, 0),
                tid(1000, 1),
                x(1000, 100),
                y(1000, 200),
                syn(1000),
                slot(1100, 1),
                tid(1100, 2),
                x(1100, 300),
                y(1100, 400),
                syn(1100),
                slot(1200, 0),
                x(1200, 150),
                syn(1200),
            ],
        );
        let frames = harness.frames();
        assert_eq!(frames.len(), 3);
        // Frame 3: only slot 0's x changed; slot 1 must stay exactly as
        // committed (inherited), and slot 0's y must be inherited too.
        assert_eq!(
            frames[2].contacts,
            vec![
                contact(1, 0, ContactState::Active, Some(mm(150)), Some(mm(200))),
                contact(2, 1, ContactState::Active, Some(mm(300)), Some(mm(400))),
            ]
        );
    }

    #[test]
    fn tracking_id_replacement_ends_old_contact_implicitly() {
        let mut harness = Harness::configured(type_b_descriptor());
        feed_all(
            &mut harness.decoder,
            &[
                slot(1000, 0),
                tid(1000, 1),
                x(1000, 100),
                y(1000, 200),
                syn(1000),
                tid(1100, 2),
                x(1100, 300),
                y(1100, 400),
                syn(1100),
            ],
        );
        let frames = harness.frames();
        assert_eq!(frames.len(), 2);
        // The replaced frame shows only the new tracking id as Began; the old
        // contact ended implicitly and must not appear (a frame cannot carry
        // two contacts on one slot).
        assert_eq!(
            frames[1].contacts,
            vec![contact(
                2,
                0,
                ContactState::Began,
                Some(mm(300)),
                Some(mm(400))
            )]
        );
        assert!(frames[1]
            .diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::TrackingIdReplaced && d.message.contains("1 -> 2")));
    }

    #[test]
    fn unupdated_fields_inherit_from_committed_state() {
        let mut harness = Harness::configured(descriptor_with_pressure());
        feed_all(
            &mut harness.decoder,
            &[
                slot(1000, 0),
                tid(1000, 1),
                x(1000, 100),
                y(1000, 200),
                pressure(1000, 50),
                syn(1000),
                y(1100, 210),
                pressure(1100, 60),
                syn(1100),
            ],
        );
        let frames = harness.frames();
        assert_eq!(frames.len(), 2);
        assert_eq!(
            frames[0].contacts,
            vec![Contact {
                tracking_id: 1,
                slot: 0,
                x_mm: Some(mm(100)),
                y_mm: Some(mm(200)),
                pressure: Some(0.5),
                major_mm: None,
                minor_mm: None,
                orientation: None,
                state: ContactState::Began,
            }]
        );
        // Frame 2 updates y and pressure; x is inherited from frame 1.
        assert_eq!(
            frames[1].contacts,
            vec![Contact {
                tracking_id: 1,
                slot: 0,
                x_mm: Some(mm(100)),
                y_mm: Some(mm(210)),
                pressure: Some(0.6),
                major_mm: None,
                minor_mm: None,
                orientation: None,
                state: ContactState::Active,
            }]
        );
    }

    #[test]
    fn incomplete_new_contact_is_held_then_published_with_diagnostics() {
        let mut harness = Harness::configured(type_b_descriptor());
        feed_all(
            &mut harness.decoder,
            &[
                slot(1000, 0),
                tid(1000, 1),
                x(1000, 100),
                syn(1000),
                y(1100, 200),
                syn(1100),
            ],
        );
        let frames = harness.frames();
        assert_eq!(frames.len(), 2);
        // Frame 1: only X was reported; the contact is held (not published)
        // and the warning is attached to this frame.
        assert!(frames[0].contacts.is_empty());
        assert!(
            frames[0]
                .diagnostics
                .iter()
                .any(|d| d.code == DiagnosticCode::IncompleteNewContact
                    && d.frame_sequence == Some(1))
        );
        // Frame 2: both coordinates are known; the contact is published with
        // the delayed-publication notice.
        assert_eq!(
            frames[1].contacts,
            vec![contact(
                1,
                0,
                ContactState::Began,
                Some(mm(100)),
                Some(mm(200))
            )]
        );
        assert!(frames[1]
            .diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::DelayedNewContact && d.frame_sequence == Some(2)));
    }

    #[test]
    fn physical_buttons_commit_atomically_with_frame() {
        let mut harness = Harness::configured(type_b_descriptor());
        feed_all(
            &mut harness.decoder,
            &[
                slot(1000, 0),
                tid(1000, 1),
                x(1000, 100),
                y(1000, 200),
                syn(1000),
                btn(1100, BTN_LEFT, 1),
                x(1100, 120),
                syn(1100),
                btn(1200, BTN_LEFT, 0),
                syn(1200),
            ],
        );
        let frames = harness.frames();
        assert_eq!(frames.len(), 3);
        // The button press and the contact move land in the same frame.
        assert_eq!(
            frames[1].physical_buttons,
            PhysicalButtons::new(true, false, false)
        );
        assert_eq!(
            frames[1].contacts,
            vec![contact(
                1,
                0,
                ContactState::Active,
                Some(mm(120)),
                Some(mm(200))
            )]
        );
        assert_eq!(frames[2].physical_buttons, PhysicalButtons::NONE);
    }

    #[test]
    fn invalid_slot_selection_fails_closed_until_a_valid_selection() {
        // An out-of-range ABS_MT_SLOT revokes slot selection (fail closed):
        // slot-scoped events must be ignored, never redirected to the
        // previous slot, and a later valid selection resumes normal decoding.
        let mut harness = Harness::configured(type_b_descriptor());
        feed_all(
            &mut harness.decoder,
            &[
                slot(1000, 0),
                tid(1000, 1),
                x(1000, 100),
                y(1000, 200),
                syn(1000),
                // Out-of-range selection: diagnosed, selection revoked.
                slot(1100, 99),
                // These belong to an invalid selection: ignored, and they
                // must NOT modify slot 0.
                x(1100, 150),
                tid(1100, 2),
                syn(1100),
                // A valid selection resumes normal decoding.
                slot(1200, 0),
                x(1200, 150),
                syn(1200),
            ],
        );
        let frames = harness.frames();
        assert_eq!(frames.len(), 3);
        // Frame 2: slot 0 is unchanged (x stays 100, y stays 200); the
        // ignored x/tid events did not leak into it.
        assert_eq!(
            frames[1].contacts,
            vec![contact(
                1,
                0,
                ContactState::Active,
                Some(mm(100)),
                Some(mm(200))
            )]
        );
        assert!(
            frames[1]
                .diagnostics
                .iter()
                .any(|d| d.code == DiagnosticCode::SlotOutOfRange
                    && d.level == DiagnosticLevel::Error)
        );
        let ignored = frames[1]
            .diagnostics
            .iter()
            .filter(|d| d.code == DiagnosticCode::InvalidEventOrder)
            .count();
        assert_eq!(ignored, 2, "the ignored x and tid events are diagnosed");
        // Frame 3: a valid selection resumes; slot 0 x updates to 150.
        assert_eq!(
            frames[2].contacts,
            vec![contact(
                1,
                0,
                ContactState::Active,
                Some(mm(150)),
                Some(mm(200))
            )]
        );
        assert!(frames[2].diagnostics.is_empty());
    }

    #[test]
    fn tracking_id_end_without_contact_is_diagnosed() {
        let mut harness = Harness::configured(type_b_descriptor());
        feed_all(&mut harness.decoder, &[tid(1000, -1), syn(1000)]);
        let frames = harness.frames();
        assert_eq!(frames.len(), 1);
        assert!(frames[0].contacts.is_empty());
        assert!(frames[0]
            .diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::InvalidEventOrder));
    }

    #[test]
    fn field_event_on_empty_slot_is_diagnosed() {
        let mut harness = Harness::configured(type_b_descriptor());
        // A position event before any tracking id arrives while no lifecycle
        // is live: it is diagnosed and ignored (M3 review R2, re-review 1) —
        // it must not leak into a later begin, so the contact stays held.
        feed_all(
            &mut harness.decoder,
            &[x(1000, 150), tid(1000, 5), syn(1000)],
        );
        let frames = harness.frames();
        assert_eq!(frames.len(), 1);
        assert!(frames[0]
            .diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::InvalidEventOrder));
        // Held: Y is still missing.
        assert!(frames[0].contacts.is_empty());
    }

    #[test]
    fn empty_syn_report_publishes_empty_frame() {
        let mut harness = Harness::configured(type_b_descriptor());
        feed_all(&mut harness.decoder, &[syn(1000)]);
        let frames = harness.frames();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].sequence, 1);
        assert!(frames[0].contacts.is_empty());
        assert_eq!(frames[0].physical_buttons, PhysicalButtons::NONE);
        assert!(!frames[0].discontinuity);
    }

    #[test]
    fn missing_resolution_keeps_coordinates_unnormalized_with_diagnostics() {
        let mut harness = Harness::configured(descriptor_without_resolution());
        feed_all(
            &mut harness.decoder,
            &[
                slot(1000, 0),
                tid(1000, 3),
                x(1000, 500),
                y(1000, 400),
                syn(1000),
                tid(1100, -1),
                syn(1100),
            ],
        );
        let frames = harness.frames();
        assert_eq!(frames.len(), 2);
        // The contact is published (both raw axes were reported) but its
        // coordinates stay unnormalized, with a MissingAxisResolution
        // diagnostic per unresolvable axis.
        assert_eq!(
            frames[0].contacts,
            vec![contact(3, 0, ContactState::Began, None, None)]
        );
        let missing_codes: Vec<u16> = frames[0]
            .diagnostics
            .iter()
            .filter(|d| d.code == DiagnosticCode::MissingAxisResolution)
            .map(|d| {
                // The message embeds the ABS code; assert the count instead
                // of parsing.
                let _ = d;
                1
            })
            .collect();
        assert_eq!(missing_codes.len(), 2);
    }

    #[test]
    fn contact_beginning_and_ending_within_one_frame_is_not_published() {
        let mut harness = Harness::configured(type_b_descriptor());
        feed_all(
            &mut harness.decoder,
            &[
                slot(1000, 0),
                tid(1000, 5),
                x(1000, 100),
                y(1000, 200),
                tid(1000, -1),
                syn(1000),
            ],
        );
        let frames = harness.frames();
        assert_eq!(frames.len(), 1);
        assert!(frames[0].contacts.is_empty());
        assert!(frames[0]
            .diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::InvalidEventOrder
                && d.message.contains("began and ended")));
    }

    #[test]
    fn ended_contact_that_was_never_published_is_not_published() {
        // The contact begins, stays incomplete (held), and then ends: the
        // consumer never saw it begin, so no Ended contact is published.
        let mut harness = Harness::configured(type_b_descriptor());
        feed_all(
            &mut harness.decoder,
            &[
                slot(1000, 0),
                tid(1000, 5),
                x(1000, 100),
                syn(1000),
                tid(1100, -1),
                syn(1100),
            ],
        );
        let frames = harness.frames();
        assert_eq!(frames.len(), 2);
        assert!(frames[0].contacts.is_empty());
        assert!(frames[1].contacts.is_empty());
    }

    #[test]
    fn syn_dropped_ignores_incremental_events_until_boundary() {
        let mut harness = Harness::configured(type_b_descriptor());
        harness
            .decoder
            .set_resync_source(Box::new(MockResync::Ok(KernelStateSnapshot::default())));
        feed_all(
            &mut harness.decoder,
            &[
                slot(1000, 0),
                tid(1000, 7),
                x(1000, 100),
                y(1000, 100),
                syn(1000),
                dropped(1100),
                // Incremental events between the drop and the boundary must
                // be ignored: they must not appear in the discontinuity frame.
                slot(1100, 1),
                tid(1100, 99),
                x(1100, 999),
                y(1100, 999),
                syn(1200),
            ],
        );
        let frames = harness.frames();
        assert_eq!(frames.len(), 2);
        // The discontinuity frame must reflect only the snapshot (empty
        // here), never the ignored slot-1 contact.
        assert!(frames[1].discontinuity);
        assert!(frames[1].contacts.is_empty());
        assert!(frames[1]
            .diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::DecodeRecovered));
        assert_eq!(harness.decoder.sync_state(), SyncState::Normal);
    }

    fn snapshot_with(contact: SlotSnapshot) -> KernelStateSnapshot {
        KernelStateSnapshot::new(PhysicalButtons::NONE, vec![contact])
    }

    #[test]
    fn successful_resync_publishes_discontinuity_frame_and_returns_to_normal() {
        let snapshot = snapshot_with(SlotSnapshot {
            slot: 0,
            tracking_id: 7,
            position_x: Some(RawAxis::new(110)),
            position_y: Some(RawAxis::new(110)),
            ..Default::default()
        });
        let mut harness = Harness::configured(type_b_descriptor());
        harness
            .decoder
            .set_resync_source(Box::new(MockResync::Ok(snapshot)));
        feed_all(
            &mut harness.decoder,
            &[
                slot(1000, 0),
                tid(1000, 7),
                x(1000, 100),
                y(1000, 100),
                syn(1000),
                dropped(1100),
                syn(1200),
                slot(1300, 0),
                tid(1300, 7),
                x(1300, 120),
                y(1300, 120),
                syn(1300),
            ],
        );
        let frames = harness.frames();
        assert_eq!(frames.len(), 3);
        // Discontinuity frame: complete state from the snapshot, all Began.
        assert!(frames[1].discontinuity);
        assert_eq!(frames[1].sequence, 2);
        assert_eq!(
            frames[1].contacts,
            vec![contact(
                7,
                0,
                ContactState::Began,
                Some(mm(110)),
                Some(mm(110))
            )]
        );
        assert!(frames[1]
            .diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::DecodeRecovered));
        // The next frame marks the resynced contact Active, not Began again.
        assert_eq!(
            frames[2].contacts,
            vec![contact(
                7,
                0,
                ContactState::Active,
                Some(mm(120)),
                Some(mm(120))
            )]
        );
        assert_eq!(harness.decoder.sync_state(), SyncState::Normal);
    }

    /// M4 review R6: after a successful resync the decoder reports
    /// `just_resynced() == true` for exactly the feed call that installed the
    /// snapshot; the flag resets on the next feed so the runtime knows which
    /// read batch still holds pre-snapshot events.
    #[test]
    fn just_resynced_flags_the_feed_that_installed_the_snapshot() {
        let snapshot = snapshot_with(SlotSnapshot {
            slot: 0,
            tracking_id: 7,
            position_x: Some(RawAxis::new(110)),
            position_y: Some(RawAxis::new(110)),
            ..Default::default()
        });
        let mut harness = Harness::configured(type_b_descriptor());
        harness
            .decoder
            .set_resync_source(Box::new(MockResync::Ok(snapshot)));
        assert!(!harness.decoder.just_resynced());

        // Normal events do not resync.
        harness.decoder.feed(slot(1000, 0)).unwrap();
        assert!(!harness.decoder.just_resynced());
        harness.decoder.feed(tid(1000, 7)).unwrap();
        harness.decoder.feed(x(1000, 100)).unwrap();
        harness.decoder.feed(y(1000, 100)).unwrap();
        harness.decoder.feed(syn(1000)).unwrap();
        assert!(!harness.decoder.just_resynced());

        // The recovery boundary SYN_REPORT is the feed that resyncs.
        harness.decoder.feed(dropped(1100)).unwrap();
        assert!(!harness.decoder.just_resynced());
        harness.decoder.feed(syn(1200)).unwrap();
        assert!(harness.decoder.just_resynced());

        // The flag is reset by the next feed, whatever it contains.
        harness.decoder.feed(syn(1300)).unwrap();
        assert!(!harness.decoder.just_resynced());
    }

    #[test]
    fn failed_resync_degrades_and_stops_emitting_frames() {
        let mut harness = Harness::configured(type_b_descriptor());
        harness
            .decoder
            .set_resync_source(Box::new(MockResync::Err("ioctl failed")));
        feed_all(
            &mut harness.decoder,
            &[
                slot(1000, 0),
                tid(1000, 7),
                x(1000, 100),
                y(1000, 100),
                syn(1000),
                dropped(1100),
            ],
        );
        assert_eq!(
            harness.decoder.sync_state(),
            SyncState::DroppedAwaitingBoundary
        );
        let err = harness.decoder.feed(syn(1200)).unwrap_err();
        assert!(matches!(err, DecodeError::ResyncFailed(ref m) if m.contains("ioctl failed")));
        assert_eq!(harness.decoder.sync_state(), SyncState::Degraded);
        let frames_before = harness.frames().len();
        // A degraded decoder never emits a trusted frame again and rejects
        // every further feed.
        assert!(matches!(
            harness.decoder.feed(syn(1300)),
            Err(DecodeError::Degraded)
        ));
        assert!(matches!(
            harness.decoder.feed(x(1300, 5)),
            Err(DecodeError::Degraded)
        ));
        assert_eq!(harness.frames().len(), frames_before);
    }

    #[test]
    fn missing_resync_source_fails_recovery() {
        let mut harness = Harness::configured(type_b_descriptor());
        feed_all(&mut harness.decoder, &[dropped(1000)]);
        let err = harness.decoder.feed(syn(1100)).unwrap_err();
        assert!(matches!(
            err,
            DecodeError::ResyncFailed(ref m) if m.contains("no resync source")
        ));
        assert_eq!(harness.decoder.sync_state(), SyncState::Degraded);
    }

    #[test]
    fn resync_queries_snapshot_exactly_once_at_the_boundary() {
        let counter = Rc::new(Cell::new(0));
        let snapshot = snapshot_with(SlotSnapshot {
            slot: 0,
            tracking_id: 7,
            position_x: Some(RawAxis::new(100)),
            position_y: Some(RawAxis::new(100)),
            ..Default::default()
        });
        let mut harness = Harness::configured(type_b_descriptor());
        harness
            .decoder
            .set_resync_source(Box::new(MockResync::Counting(counter.clone(), snapshot)));
        feed_all(
            &mut harness.decoder,
            &[dropped(1000), syn(1100), syn(1200), syn(1300)],
        );
        assert_eq!(counter.get(), 1, "resync must query exactly once");
        assert_eq!(harness.decoder.sync_state(), SyncState::Normal);
    }

    #[test]
    fn resync_snapshot_out_of_range_slot_degrades_without_a_frame() {
        // An out-of-range slot in the snapshot is a resync failure (M3
        // review R4): the decoder degrades and publishes no discontinuity
        // frame, and every later feed is rejected.
        let snapshot = snapshot_with(SlotSnapshot {
            slot: 99,
            tracking_id: 7,
            position_x: Some(RawAxis::new(100)),
            position_y: Some(RawAxis::new(100)),
            ..Default::default()
        });
        let mut harness = Harness::configured(type_b_descriptor());
        harness
            .decoder
            .set_resync_source(Box::new(MockResync::Ok(snapshot)));
        feed_all(&mut harness.decoder, &[dropped(1000)]);
        let err = harness.decoder.feed(syn(1100)).unwrap_err();
        assert!(matches!(
            err,
            DecodeError::ResyncFailed(ref m)
                if m.contains("slot 99") && m.contains("outside the device's slot range")
        ));
        assert_eq!(harness.decoder.sync_state(), SyncState::Degraded);
        assert!(
            harness.frames().is_empty(),
            "no trusted frame may be emitted"
        );
        assert!(matches!(
            harness.decoder.feed(syn(1200)),
            Err(DecodeError::Degraded)
        ));
        assert!(harness.frames().is_empty());
    }

    #[test]
    fn resync_snapshot_duplicate_slots_degrades_without_a_frame() {
        let snapshot = KernelStateSnapshot::new(
            PhysicalButtons::NONE,
            vec![
                SlotSnapshot::new(0, -1),
                SlotSnapshot {
                    slot: 0,
                    tracking_id: 7,
                    position_x: Some(RawAxis::new(100)),
                    position_y: Some(RawAxis::new(100)),
                    ..Default::default()
                },
            ],
        );
        let mut harness = Harness::configured(type_b_descriptor());
        harness
            .decoder
            .set_resync_source(Box::new(MockResync::Ok(snapshot)));
        feed_all(&mut harness.decoder, &[dropped(1000)]);
        let err = harness.decoder.feed(syn(1100)).unwrap_err();
        assert!(matches!(
            err,
            DecodeError::ResyncFailed(ref m) if m.contains("more than once")
        ));
        assert_eq!(harness.decoder.sync_state(), SyncState::Degraded);
        assert!(harness.frames().is_empty());
        assert!(matches!(
            harness.decoder.feed(syn(1200)),
            Err(DecodeError::Degraded)
        ));
    }

    #[test]
    fn resync_snapshot_active_contact_missing_coordinates_degrades() {
        // An active snapshot contact must carry both raw X and Y; a snapshot
        // without them is incomplete and must not be trusted.
        let snapshot = snapshot_with(SlotSnapshot::new(0, 7));
        let mut harness = Harness::configured(type_b_descriptor());
        harness
            .decoder
            .set_resync_source(Box::new(MockResync::Ok(snapshot)));
        feed_all(&mut harness.decoder, &[dropped(1000)]);
        let err = harness.decoder.feed(syn(1100)).unwrap_err();
        assert!(matches!(
            err,
            DecodeError::ResyncFailed(ref m)
                if m.contains("without required raw X/Y coordinates")
        ));
        assert_eq!(harness.decoder.sync_state(), SyncState::Degraded);
        assert!(harness.frames().is_empty());
        assert!(matches!(
            harness.decoder.feed(syn(1200)),
            Err(DecodeError::Degraded)
        ));
    }

    #[test]
    fn resync_snapshot_invalid_tracking_id_degrades() {
        // Only tracking ids >= 0 (active) or exactly -1 (empty) are valid in
        // a kernel snapshot; anything below -1 is invalid.
        let snapshot = snapshot_with(SlotSnapshot::new(0, -2));
        let mut harness = Harness::configured(type_b_descriptor());
        harness
            .decoder
            .set_resync_source(Box::new(MockResync::Ok(snapshot)));
        feed_all(&mut harness.decoder, &[dropped(1000)]);
        let err = harness.decoder.feed(syn(1100)).unwrap_err();
        assert!(matches!(
            err,
            DecodeError::ResyncFailed(ref m) if m.contains("invalid tracking id")
        ));
        assert_eq!(harness.decoder.sync_state(), SyncState::Degraded);
        assert!(harness.frames().is_empty());
        assert!(matches!(
            harness.decoder.feed(syn(1200)),
            Err(DecodeError::Degraded)
        ));
    }

    #[test]
    fn resync_snapshot_buttons_are_published() {
        let mut snapshot = snapshot_with(SlotSnapshot {
            slot: 0,
            tracking_id: 7,
            position_x: Some(RawAxis::new(100)),
            position_y: Some(RawAxis::new(100)),
            ..Default::default()
        });
        snapshot.physical_buttons = PhysicalButtons::new(true, false, false);
        let mut harness = Harness::configured(type_b_descriptor());
        harness
            .decoder
            .set_resync_source(Box::new(MockResync::Ok(snapshot)));
        feed_all(&mut harness.decoder, &[dropped(1000), syn(1100)]);
        let frames = harness.frames();
        assert_eq!(frames.len(), 1);
        assert_eq!(
            frames[0].physical_buttons,
            PhysicalButtons::new(true, false, false)
        );
    }

    #[test]
    fn feed_before_configure_is_rejected() {
        let mut harness = Harness::new();
        assert!(matches!(
            harness.decoder.feed(syn(1000)),
            Err(DecodeError::NotConfigured)
        ));
    }

    #[test]
    fn configure_twice_is_rejected() {
        let mut harness = Harness::new();
        harness.decoder.configure(type_b_descriptor()).unwrap();
        assert!(matches!(
            harness.decoder.configure(type_b_descriptor()),
            Err(DecodeError::AlreadyConfigured)
        ));
    }

    #[test]
    fn configure_rejects_non_type_b_device() {
        let mut device = type_b_descriptor();
        device.supports_type_b_mt = false;
        let mut harness = Harness::new();
        assert!(matches!(
            harness.decoder.configure(device),
            Err(DecodeError::InvalidDevice(_))
        ));
    }

    #[test]
    fn configure_rejects_missing_slot_count() {
        let mut device = type_b_descriptor();
        device.slot_count = None;
        let mut harness = Harness::new();
        assert!(matches!(
            harness.decoder.configure(device),
            Err(DecodeError::InvalidDevice(_))
        ));
    }

    #[test]
    fn tracking_id_below_minus_one_is_diagnosed_and_ignored() {
        // Exactly -1 ends a contact; any value below -1 is invalid and must
        // neither end nor replace a contact (M3 review R2).
        let mut harness = Harness::configured(type_b_descriptor());
        feed_all(
            &mut harness.decoder,
            &[
                slot(1000, 0),
                tid(1000, 1),
                x(1000, 100),
                y(1000, 200),
                syn(1000),
                tid(1100, -2),
                syn(1100),
            ],
        );
        let frames = harness.frames();
        assert_eq!(frames.len(), 2);
        // The contact continues: -2 did not end it and did not replace it.
        assert_eq!(
            frames[1].contacts,
            vec![contact(
                1,
                0,
                ContactState::Active,
                Some(mm(100)),
                Some(mm(200))
            )]
        );
        assert!(frames[1]
            .diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::InvalidEventOrder && d.message.contains("-2")));
    }

    #[test]
    fn end_then_begin_in_one_cycle_leaves_new_lifecycle_at_boundary() {
        // end(-1) -> begin(new) in one SYN cycle: the old contact ends
        // implicitly and the new tracking id is the lifecycle live at the
        // boundary (published as Began). The new lifecycle must not inherit
        // the old contact's coordinates (M3 review R2).
        let mut harness = Harness::configured(type_b_descriptor());
        feed_all(
            &mut harness.decoder,
            &[
                slot(1000, 0),
                tid(1000, 1),
                x(1000, 100),
                y(1000, 200),
                syn(1000),
                tid(1100, -1),
                tid(1100, 2),
                x(1100, 300),
                y(1100, 400),
                syn(1100),
            ],
        );
        let frames = harness.frames();
        assert_eq!(frames.len(), 2);
        assert_eq!(
            frames[1].contacts,
            vec![contact(
                2,
                0,
                ContactState::Began,
                Some(mm(300)),
                Some(mm(400))
            )]
        );
        // No prior-contact fields leak: the new lifecycle is at (300, 400),
        // not the old (100, 200).
        assert!(frames[1].diagnostics.is_empty());
    }

    #[test]
    fn repeated_tracking_id_begin_is_a_noop() {
        // A kernel driver may resend the same tracking id within a cycle; the
        // repeated begin must not replace or end the contact (M3 review R2).
        let mut harness = Harness::configured(type_b_descriptor());
        feed_all(
            &mut harness.decoder,
            &[
                slot(1000, 0),
                tid(1000, 1),
                x(1000, 100),
                y(1000, 200),
                syn(1000),
                tid(1100, 1),
                x(1100, 120),
                syn(1100),
            ],
        );
        let frames = harness.frames();
        assert_eq!(frames.len(), 2);
        assert_eq!(
            frames[1].contacts,
            vec![contact(
                1,
                0,
                ContactState::Active,
                Some(mm(120)),
                Some(mm(200))
            )]
        );
        assert!(frames[1].diagnostics.is_empty());
    }

    #[test]
    fn multiple_tracking_replacements_in_one_cycle_keep_only_final_lifecycle() {
        // begin(1) -> begin(2) -> begin(3) in one cycle: each replacement is
        // diagnosed, and only the final lifecycle (3) is live at the
        // boundary; the old contacts end implicitly (M3 review R2).
        let mut harness = Harness::configured(type_b_descriptor());
        feed_all(
            &mut harness.decoder,
            &[
                slot(1000, 0),
                tid(1000, 1),
                x(1000, 100),
                y(1000, 200),
                syn(1000),
                tid(1100, 2),
                tid(1100, 3),
                x(1100, 300),
                y(1100, 400),
                syn(1100),
            ],
        );
        let frames = harness.frames();
        assert_eq!(frames.len(), 2);
        assert_eq!(
            frames[1].contacts,
            vec![contact(
                3,
                0,
                ContactState::Began,
                Some(mm(300)),
                Some(mm(400))
            )]
        );
        let replaced: Vec<&str> = frames[1]
            .diagnostics
            .iter()
            .filter(|d| d.code == DiagnosticCode::TrackingIdReplaced)
            .map(|d| d.message.as_str())
            .collect();
        assert_eq!(replaced.len(), 2);
        assert!(replaced.iter().any(|m| m.contains("1 -> 2")));
        assert!(replaced.iter().any(|m| m.contains("2 -> 3")));
        // The final lifecycle must not inherit the old contact's coordinates.
        assert_eq!(frames[1].contacts[0].x_mm, Some(mm(300)));
        assert_eq!(frames[1].contacts[0].y_mm, Some(mm(400)));
    }

    #[test]
    fn field_before_replacement_does_not_complete_the_new_contact() {
        // A field that arrives while the old lifecycle is still active belongs
        // to that lifecycle; when the tracking id is replaced, it must be
        // discarded, not applied to the new contact (M3 review R2, re-review
        // 1). `x(150) -> tid(2) -> y(400)` must leave lifecycle 2 with only Y:
        // it is held as incomplete, and is later completed by a *new* X, never
        // by the old lifecycle's X.
        let mut harness = Harness::configured(type_b_descriptor());
        feed_all(
            &mut harness.decoder,
            &[
                slot(1000, 0),
                tid(1000, 1),
                x(1000, 100),
                y(1000, 200),
                syn(1000),
                // x(150) belongs to lifecycle 1; tid(2) starts a clean
                // lifecycle and discards it; y(400) belongs to lifecycle 2.
                slot(1100, 0),
                x(1100, 150),
                tid(1100, 2),
                y(1100, 400),
                syn(1100),
                // Lifecycle 2 is completed with a fresh X.
                x(1200, 350),
                syn(1200),
            ],
        );
        let frames = harness.frames();
        assert_eq!(frames.len(), 3);
        // Frame 2: lifecycle 2 has only Y after its begin, so it is held; the
        // old X (150) must not complete it.
        assert!(
            frames[1].contacts.is_empty(),
            "the old lifecycle's X must not complete the new contact"
        );
        assert!(frames[1]
            .diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::TrackingIdReplaced && d.message.contains("1 -> 2")));
        assert!(frames[1]
            .diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::IncompleteNewContact));
        // Frame 3: the new lifecycle publishes only with the X that arrived
        // after its begin — (350, 400), never (150, 400).
        assert_eq!(
            frames[2].contacts,
            vec![contact(
                2,
                0,
                ContactState::Began,
                Some(mm(350)),
                Some(mm(400))
            )]
        );
    }

    #[test]
    fn field_after_end_does_not_alter_the_ended_contact() {
        // A field arriving after `tid(-1)` must be diagnosed and ignored: it
        // belongs to no lifecycle and must not alter the prior Ended contact
        // (M3 review R2, re-review 1).
        let mut harness = Harness::configured(type_b_descriptor());
        feed_all(
            &mut harness.decoder,
            &[
                slot(1000, 0),
                tid(1000, 1),
                x(1000, 100),
                y(1000, 200),
                syn(1000),
                tid(1100, -1),
                // Arrives after the end: ignored, must not move the contact.
                x(1100, 150),
                syn(1100),
            ],
        );
        let frames = harness.frames();
        assert_eq!(frames.len(), 2);
        assert_eq!(
            frames[1].contacts,
            vec![contact(
                1,
                0,
                ContactState::Ended,
                Some(mm(100)),
                Some(mm(200))
            )],
            "the post-end X must not alter the Ended contact's position"
        );
        assert!(frames[1]
            .diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::InvalidEventOrder
                && d.message.contains("after its contact ended")));
    }

    #[test]
    fn field_after_end_does_not_leak_into_a_later_begin() {
        // The ignored post-end field must also not show up in a contact that
        // begins later in the same cycle: `tid(-1) -> x(150) -> tid(2) ->
        // y(400)` leaves lifecycle 2 with only Y, and a later X completes it
        // at its own value, never at 150 (M3 review R2, re-review 1).
        let mut harness = Harness::configured(type_b_descriptor());
        feed_all(
            &mut harness.decoder,
            &[
                slot(1000, 0),
                tid(1000, 1),
                x(1000, 100),
                y(1000, 200),
                syn(1000),
                tid(1100, -1),
                x(1100, 150),
                tid(1100, 2),
                y(1100, 400),
                syn(1100),
                x(1200, 350),
                syn(1200),
            ],
        );
        let frames = harness.frames();
        assert_eq!(frames.len(), 3);
        assert!(
            frames[1].contacts.is_empty(),
            "lifecycle 2 has only Y after its begin; it must be held"
        );
        assert!(frames[1]
            .diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::InvalidEventOrder
                && d.message.contains("after its contact ended")));
        assert_eq!(
            frames[2].contacts,
            vec![contact(
                2,
                0,
                ContactState::Began,
                Some(mm(350)),
                Some(mm(400))
            )],
            "the post-end X (150) must not leak into the later contact"
        );
    }

    #[test]
    fn interleaved_fields_across_multiple_replacements_stay_with_their_lifecycle() {
        // x(110) belongs to lifecycle 1, y(210) to lifecycle 2, and x/y(310,
        // 410) to lifecycle 3: each replacement starts a clean lifecycle that
        // discards the previous one's fields, so the final contact publishes
        // exactly the fields that arrived after its own begin (M3 review R2,
        // re-review 1).
        let mut harness = Harness::configured(type_b_descriptor());
        feed_all(
            &mut harness.decoder,
            &[
                slot(1000, 0),
                tid(1000, 1),
                x(1000, 100),
                y(1000, 200),
                syn(1000),
                slot(1100, 0),
                x(1100, 110),
                tid(1100, 2),
                y(1100, 210),
                tid(1100, 3),
                x(1100, 310),
                y(1100, 410),
                syn(1100),
            ],
        );
        let frames = harness.frames();
        assert_eq!(frames.len(), 2);
        assert_eq!(
            frames[1].contacts,
            vec![contact(
                3,
                0,
                ContactState::Began,
                Some(mm(310)),
                Some(mm(410))
            )]
        );
        let replaced: Vec<&str> = frames[1]
            .diagnostics
            .iter()
            .filter(|d| d.code == DiagnosticCode::TrackingIdReplaced)
            .map(|d| d.message.as_str())
            .collect();
        assert_eq!(replaced.len(), 2);
        assert!(replaced.iter().any(|m| m.contains("1 -> 2")));
        assert!(replaced.iter().any(|m| m.contains("2 -> 3")));
        // Neither lifecycle 1's X nor lifecycle 2's Y may leak into 3.
        assert_eq!(frames[1].contacts[0].x_mm, Some(mm(310)));
        assert_eq!(frames[1].contacts[0].y_mm, Some(mm(410)));
    }

    #[test]
    fn incomplete_interleaved_replacement_chain_is_held_until_complete() {
        // The same interleaved chain as above, but lifecycle 3 receives only
        // X before the boundary: it must be held (incomplete), and the Y that
        // arrives later completes it at its own value — lifecycle 2's Y (210)
        // must not leak in (M3 review R2, re-review 1).
        let mut harness = Harness::configured(type_b_descriptor());
        feed_all(
            &mut harness.decoder,
            &[
                slot(1000, 0),
                tid(1000, 1),
                x(1000, 100),
                y(1000, 200),
                syn(1000),
                slot(1100, 0),
                x(1100, 110),
                tid(1100, 2),
                y(1100, 210),
                tid(1100, 3),
                x(1100, 310),
                syn(1100),
                y(1200, 410),
                syn(1200),
            ],
        );
        let frames = harness.frames();
        assert_eq!(frames.len(), 3);
        assert!(
            frames[1].contacts.is_empty(),
            "lifecycle 3 has only X after its begin; it must be held"
        );
        assert!(frames[1]
            .diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::IncompleteNewContact));
        assert_eq!(
            frames[2].contacts,
            vec![contact(
                3,
                0,
                ContactState::Began,
                Some(mm(310)),
                Some(mm(410))
            )]
        );
    }

    #[test]
    fn many_replacements_in_one_cycle_are_bounded_and_summarized() {
        // A replay-controlled stream can emit arbitrarily many tracking-id
        // replacements without a SYN_REPORT; the decoder's per-slot state
        // must stay bounded. Only the first MAX_TRACKING_REPLACEMENTS steps
        // are retained for diagnostics; the rest are counted and summarized
        // (M3 review R2, re-review 1).
        let mut events: Vec<RawEvent> = vec![
            slot(1000, 0),
            tid(1000, 1),
            x(1000, 100),
            y(1000, 200),
            syn(1000),
        ];
        for id in 2..=21 {
            events.push(tid(1100, id));
        }
        events.push(x(1100, 500));
        events.push(y(1100, 600));
        events.push(syn(1100));

        let mut harness = Harness::configured(type_b_descriptor());
        feed_all(&mut harness.decoder, &events);
        let frames = harness.frames();
        assert_eq!(frames.len(), 2);
        assert_eq!(
            frames[1].contacts,
            vec![contact(
                21,
                0,
                ContactState::Began,
                Some(mm(500)),
                Some(mm(600))
            )]
        );
        // 20 replacement steps: the first 16 are reported individually, the
        // remaining 4 are summarized in one diagnostic.
        let replaced: Vec<&str> = frames[1]
            .diagnostics
            .iter()
            .filter(|d| d.code == DiagnosticCode::TrackingIdReplaced)
            .map(|d| d.message.as_str())
            .collect();
        assert_eq!(
            replaced.len(),
            MAX_TRACKING_REPLACEMENTS + 1,
            "16 individual steps plus one overflow summary"
        );
        assert!(replaced.iter().any(|m| m.contains("4 additional times")));
        // The final lifecycle must still carry only its own fields.
        assert_eq!(frames[1].contacts[0].x_mm, Some(mm(500)));
        assert_eq!(frames[1].contacts[0].y_mm, Some(mm(600)));
    }

    #[test]
    fn touch_major_minor_use_delta_conversion_not_position_origin() {
        // Touch major/minor are contact lengths: with a non-zero axis
        // minimum, raw 150 must convert to 150/100 = 1.5 mm, not
        // (150 - 100)/100 = 0.5 mm (M3 review R3).
        let mut harness = Harness::configured(descriptor_with_nonzero_min_lengths());
        feed_all(
            &mut harness.decoder,
            &[
                slot(1000, 0),
                tid(1000, 1),
                x(1000, 300),
                y(1000, 400),
                touch_major(1000, 150),
                touch_minor(1000, 50),
                syn(1000),
            ],
        );
        let frames = harness.frames();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].contacts.len(), 1);
        let contact = &frames[0].contacts[0];
        assert_eq!(contact.x_mm, Some(mm(300)));
        assert_eq!(contact.y_mm, Some(mm(400)));
        assert_eq!(
            contact.major_mm,
            Some(length_mm(150)),
            "major must use delta conversion (1.5 mm), not position (0.5 mm)"
        );
        assert_eq!(
            contact.minor_mm,
            Some(length_mm(50)),
            "minor must use delta conversion (0.5 mm); position conversion would be negative"
        );
        assert!(frames[0].diagnostics.is_empty());
    }

    #[test]
    fn resync_snapshot_lengths_use_delta_conversion_not_position_origin() {
        // The snapshot/resync path must normalize touch lengths with the same
        // delta conversion as the pending path (M3 review R3).
        let snapshot = snapshot_with(SlotSnapshot {
            slot: 0,
            tracking_id: 7,
            position_x: Some(RawAxis::new(300)),
            position_y: Some(RawAxis::new(400)),
            touch_major: Some(RawAxis::new(150)),
            touch_minor: Some(RawAxis::new(50)),
            ..Default::default()
        });
        let mut harness = Harness::configured(descriptor_with_nonzero_min_lengths());
        harness
            .decoder
            .set_resync_source(Box::new(MockResync::Ok(snapshot)));
        feed_all(&mut harness.decoder, &[dropped(1000), syn(1100)]);
        let frames = harness.frames();
        assert_eq!(frames.len(), 1);
        assert!(frames[0].discontinuity);
        assert_eq!(frames[0].contacts.len(), 1);
        let contact = &frames[0].contacts[0];
        assert_eq!(contact.x_mm, Some(mm(300)));
        assert_eq!(contact.y_mm, Some(mm(400)));
        assert_eq!(contact.major_mm, Some(length_mm(150)));
        assert_eq!(contact.minor_mm, Some(length_mm(50)));
    }

    #[test]
    fn replay_finish_rejects_unresolved_synchronization_loss() {
        use touchpad_trace::ReplaySink;
        // A decoder left in DroppedAwaitingBoundary at finish must fail: the
        // trace ended with unresolved synchronization loss (M3 review R5).
        let mut harness = Harness::configured(type_b_descriptor());
        harness.decoder.feed(dropped(1000)).unwrap();
        let err = ReplaySink::finish(&mut harness.decoder).unwrap_err();
        assert!(matches!(
            err,
            crate::replay::ReplayDecodeError::UnresolvedSynchronizationLoss(
                SyncState::DroppedAwaitingBoundary
            )
        ));
        assert!(harness.frames().is_empty(), "finish must not emit a frame");
        // A decoder in Normal state finishes cleanly (an ordinary trace
        // ending between frames).
        let mut harness = Harness::configured(type_b_descriptor());
        assert!(ReplaySink::finish(&mut harness.decoder).is_ok());
    }

    #[test]
    fn configure_accepts_maximum_slot_count() {
        let mut device = type_b_descriptor();
        device.slot_count = Some(MAX_SLOT_COUNT);
        let mut harness = Harness::new();
        assert!(harness.decoder.configure(device).is_ok());
    }

    #[test]
    fn configure_rejects_slot_count_above_maximum() {
        // The first value above the documented bound must be rejected with a
        // structured InvalidDevice error before any allocation (M3 review
        // R6).
        let mut device = type_b_descriptor();
        device.slot_count = Some(MAX_SLOT_COUNT + 1);
        let mut harness = Harness::new();
        let err = harness.decoder.configure(device).unwrap_err();
        assert!(matches!(
            err,
            DecodeError::InvalidDevice(ref m)
                if m.contains("slot_count") && m.contains("maximum")
        ));
    }

    #[test]
    fn raw_event_from_trace_event_converts_timestamps() {
        let trace = touchpad_trace::TraceEvent::new(1, 500_000, EV_ABS, ABS_MT_POSITION_X, 42);
        let raw = RawEvent::from_trace_event(&trace).unwrap();
        assert_eq!(raw.timestamp, Monotonic::from_nanos(1_500_000_000));
        assert_eq!(raw.event_type, EV_ABS);
        assert_eq!(raw.code, ABS_MT_POSITION_X);
        assert_eq!(raw.value, 42);
        // An unrepresentable timestamp (huge sec) yields None.
        let bad = touchpad_trace::TraceEvent::new(u64::MAX, 0, EV_ABS, ABS_MT_POSITION_X, 0);
        assert!(RawEvent::from_trace_event(&bad).is_none());
    }
}
