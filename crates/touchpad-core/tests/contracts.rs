//! Public-contract tests for `touchpad-core`.
//!
//! These tests use only the crate's public API, mirroring how the future
//! `touchpad-linux` and `touchpad-trace` crates will consume it.

use std::num::NonZeroU32;

use touchpad_core::{
    raw_axis_delta_to_mm, raw_axis_position_to_mm, raw_axis_position_to_mm_with_resolution,
    AxisConversionError, AxisId, AxisInfo, Contact, ContactFrame, ContactState, DeviceDescriptor,
    DeviceProfile, DiagnosticCode, LogicalPixels, Millimeters, Monotonic, MouseButton, OutputEvent,
    OutputSink, PhysicalButtons, RawAxis, RecordingSink,
};

fn sample_frame() -> ContactFrame {
    let mut contact = Contact::new(7, 0, ContactState::Began);
    contact.x_mm = Some(Millimeters::try_new(10.0).unwrap());
    contact.y_mm = Some(Millimeters::try_new(20.0).unwrap());
    contact.pressure = Some(0.5);
    ContactFrame {
        monotonic_timestamp: Monotonic::from_nanos(1_000),
        sequence: 3,
        discontinuity: false,
        contacts: vec![contact],
        physical_buttons: PhysicalButtons::new(true, false, false),
        diagnostics: vec![],
    }
}

#[test]
fn frame_round_trips_through_json() {
    let frame = sample_frame();
    let json = serde_json::to_string(&frame).unwrap();
    let decoded: ContactFrame = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, frame);
}

#[test]
fn json_rejects_non_finite_unit_values() {
    // `NaN`/`Infinity` are not valid JSON numbers, so serde_json rejects
    // them at parse time; the units' `Deserialize` impls additionally reject
    // non-finite values that reach them through other formats (covered in
    // unit tests with raw `f32` deserializers).
    assert!(serde_json::from_str::<Millimeters>("NaN").is_err());
    assert!(serde_json::from_str::<LogicalPixels>("Infinity").is_err());
}

#[test]
fn frame_validation_detects_structural_bugs() {
    let mut frame = sample_frame();
    let mut duplicate = frame.contacts[0].clone();
    duplicate.slot = 0; // same slot as the first contact
    frame.contacts.push(duplicate);
    let diags = frame.validate();
    assert!(diags
        .iter()
        .any(|d| d.code == DiagnosticCode::DuplicateSlot));
}

#[test]
fn output_sink_contract_ordering_and_release() {
    let mut sink = RecordingSink::new();
    for event in [
        OutputEvent::ScrollBegin,
        OutputEvent::ScrollDelta {
            dx: LogicalPixels::try_new(0.0).unwrap(),
            dy: LogicalPixels::try_new(-5.0).unwrap(),
        },
        OutputEvent::ScrollEnd,
        OutputEvent::ButtonDown(MouseButton::Left),
        OutputEvent::ButtonUp(MouseButton::Left),
    ] {
        sink.submit(event).unwrap();
    }
    sink.release_all().unwrap();
    assert_eq!(sink.len(), 5);
    assert_eq!(
        sink.events()[1],
        OutputEvent::ScrollDelta {
            dx: LogicalPixels::try_new(0.0).unwrap(),
            dy: LogicalPixels::try_new(-5.0).unwrap()
        }
    );
}

#[test]
fn profile_override_is_the_only_path_when_resolution_is_missing() {
    let axis = AxisId::new(0);
    let info = AxisInfo::new(0, 1000, 0, 0, None);

    // No device resolution, no override -> explicit error, no fake millimeters.
    assert_eq!(
        raw_axis_position_to_mm(RawAxis::new(150), &info),
        Err(AxisConversionError::MissingResolution)
    );

    // The explicit profile override supplies the resolution -> conversion
    // works through the override path, with the same origin semantics.
    let profile = DeviceProfile::new("override-test")
        .with_axis_resolution(axis, NonZeroU32::new(100).unwrap());
    let resolution = profile
        .effective_resolution(axis, &info)
        .expect("profile override must supply the resolution");
    assert_eq!(
        raw_axis_position_to_mm_with_resolution(RawAxis::new(150), &info, resolution).unwrap(),
        Millimeters::try_new(1.5).unwrap()
    );
}

#[test]
fn position_conversion_honors_axis_origin_via_public_api() {
    // Absolute positions map the axis minimum to 0 mm, so a `min != 0` axis
    // does not shift the whole coordinate space (review R1).
    let info = AxisInfo::new(100, 500, 0, 0, NonZeroU32::new(100));
    assert_eq!(
        raw_axis_position_to_mm(RawAxis::new(100), &info).unwrap(),
        Millimeters::try_new(0.0).unwrap()
    );
    assert_eq!(
        raw_axis_position_to_mm(RawAxis::new(300), &info).unwrap(),
        Millimeters::try_new(2.0).unwrap()
    );
    // The profile override path keeps the same origin: only the resolution
    // is replaced, the min still maps to 0 mm.
    let no_resolution = AxisInfo::new(100, 500, 0, 0, None);
    assert_eq!(
        raw_axis_position_to_mm_with_resolution(
            RawAxis::new(100),
            &no_resolution,
            NonZeroU32::new(100).unwrap()
        )
        .unwrap(),
        Millimeters::try_new(0.0).unwrap()
    );
}

#[test]
fn delta_conversion_has_no_origin_via_public_api() {
    // Relative deltas convert by scale only; the axis origin must not leak
    // into them.
    let resolution = NonZeroU32::new(100);
    assert_eq!(
        raw_axis_delta_to_mm(RawAxis::new(150), resolution).unwrap(),
        Millimeters::try_new(1.5).unwrap()
    );
    assert_eq!(
        raw_axis_delta_to_mm(RawAxis::new(-150), resolution).unwrap(),
        Millimeters::try_new(-1.5).unwrap()
    );
    // A delta is the same physical distance regardless of the origin of the
    // axis it is derived from.
    let origin_at_100 = AxisInfo::new(100, 500, 0, 0, NonZeroU32::new(100));
    let origin_at_0 = AxisInfo::new(0, 1000, 0, 0, NonZeroU32::new(100));
    assert_eq!(
        raw_axis_delta_to_mm(RawAxis::new(150), origin_at_100.resolution).unwrap(),
        raw_axis_delta_to_mm(RawAxis::new(150), origin_at_0.resolution).unwrap()
    );
    // Missing resolution is a structured error, never a fake millimeter.
    assert_eq!(
        raw_axis_delta_to_mm(RawAxis::new(150), None),
        Err(AxisConversionError::MissingResolution)
    );
}

#[test]
fn descriptor_round_trips_through_json() {
    let mut descriptor = DeviceDescriptor::new("test touchpad", 0x1234, 0x5678);
    descriptor.slot_count = Some(12);
    descriptor.supports_type_b_mt = true;
    descriptor.axes.insert(
        AxisId::new(0),
        AxisInfo::new(0, 1000, 0, 0, NonZeroU32::new(100)),
    );
    let json = serde_json::to_string(&descriptor).unwrap();
    let decoded: DeviceDescriptor = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, descriptor);
    assert!(decoded.validate().is_empty());
}
