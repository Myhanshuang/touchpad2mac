//! # touchpad-core
//!
//! Platform-agnostic types and contracts for the Touchpad Runtime core.
//!
//! This crate MUST NOT depend on Linux, Wayland, X11, KDE, or GNOME. The
//! physical input edge is translated into a normalized [`ContactFrame`] here;
//! every interaction algorithm (pointer, scroll, tap, drag, gesture) and the
//! [`OutputSink`] contract build on these types.
//!
//! Key invariants:
//!
//! * Wall-clock time never participates in timeout or velocity math — only
//!   [`Monotonic`] timestamps do.
//! * Raw axis values and physical millimeters are distinct, non-interchangeable
//!   types; conversion is explicit and fails loudly when the device resolution
//!   is unknown (rather than pretending to produce precise millimeters).
//! * A missing optional capability must not make a whole frame unusable.
//!
//! This crate is deliberately `unsafe`-free.

#![forbid(unsafe_code)]

pub mod arbiter;
pub mod axis;
pub mod contact;
pub mod device;
pub mod diagnostic;
pub mod feel;
pub mod fidelity;
pub mod gesture;
pub mod gesture_bindings;
pub mod m10;
pub mod m11;
pub mod m12;
pub mod m13;
pub mod m14;
pub mod m15;
pub mod m16;
pub mod m17;
pub mod m18;
pub mod m19;
pub mod output;
pub mod production;
pub mod profile;
pub mod robustness;
pub mod scroll_fidelity;
pub mod settings;
pub mod three_finger_drag;
pub mod time;
pub mod units;
pub mod validation;

pub use arbiter::{
    Arbiter, ArbiterConfig, ArbiterConfigError, ArbiterError, ArbiterSink, ArbiterSinkError,
    FrameDecision, Lifecycle, LifecycleTransition, TapConfig, TapConfigError, TapDragPhase,
    TransitionError, TwoFingerConfig, TwoFingerConfigError, TwoFingerPhase,
};
pub use axis::{
    raw_axis_delta_to_mm, raw_axis_position_to_mm, raw_axis_position_to_mm_with_resolution,
    AxisConversionError, AxisInfo,
};
pub use contact::{Contact, ContactFrame, ContactState, PhysicalButtons};
pub use device::{AxisId, DeviceDescriptor};
pub use diagnostic::{Diagnostic, DiagnosticCode, DiagnosticLevel};
pub use feel::{
    feel_parameter_specs, DragFeel, FeelConfig, FeelConfigError, FeelParameterSpec, GestureFeel,
    PointerFeel, ScrollFeel, FEEL_CONFIG_VERSION,
};
pub use fidelity::{
    gain, process, scalar, smoothstep, FidelityConfig, FidelityConfigError, FidelityDeltaMm,
    FidelityError, FidelityOutcome, FidelityState,
};
pub use gesture::{
    process_gesture, GestureConfig, GestureConfigError, GestureContact, GestureDecision,
    GestureState,
};
pub use gesture_bindings::{
    route_continuous_gesture, route_three_finger_tap, GestureMapConfig, GestureMapError,
    GestureRouteState, GestureTarget, GestureTrigger, ALL_GESTURE_TARGETS, ALL_GESTURE_TRIGGERS,
    GESTURE_MAP_VERSION,
};
pub use m10::{M10Profile, M10ProfileError, M10_LINEAR_V1_NAME};
pub use m11::{M11Profile, M11ProfileError, M11_FIDELITY_V1_NAME};
pub use m12::{M12Profile, M12ProfileError, M12_SCROLL_V1_NAME};
pub use m13::{M13Profile, M13ProfileError, M13_ROBUST_V1_NAME};
pub use m14::{M14Profile, M14ProfileError, M14_GESTURES_V1_NAME};
pub use m15::{M15Profile, M15ProfileError, M15_KDE_V1_NAME};
pub use m16::{M16Profile, M16ProfileError, M16_PRODUCTION_V1_NAME};
pub use m17::{M17Profile, M17ProfileError, M17_TUNABLE_V1_NAME};
pub use m18::{M18Profile, M18ProfileError, M18_REMAP_V1_NAME};
pub use m19::{M19Profile, M19ProfileError, M19_LIVE_V1_NAME};
pub use output::{
    ContinuousGestureEvent, ContinuousGestureKind, ContinuousGesturePhase, DesktopAction,
    MouseButton, OutputError, OutputEvent, OutputFrameError, OutputSink, RecordingSink,
};
pub use production::{
    OutputAdapter, ReconnectController, ReconnectDecision, ReconnectPolicy, ReconnectPolicyError,
    RuntimeConfig, RuntimeConfigError, RuntimeConfigV1, ServiceLifecycle, ServiceState,
    ServiceTransitionError, CURRENT_RUNTIME_CONFIG_VERSION,
};
pub use profile::{DeviceProfile, DeviceQuirk};
pub use robustness::{
    filter_frame as robustness_filter_frame, ContactRole, RobustnessAvailability, RobustnessConfig,
    RobustnessConfigError, RobustnessOutcome, RobustnessState,
};
pub use scroll_fidelity::{
    begin_momentum, gain_for_speed as scroll_gain_for_speed, process_scroll, tick_momentum,
    AxisLock, MomentumOutcome, ScrollFidelityConfig, ScrollFidelityConfigError,
    ScrollFidelityError, ScrollFidelityOutcome, ScrollFidelityState,
};
pub use settings::{UserSettings, UserSettingsError, USER_SETTINGS_VERSION};
pub use three_finger_drag::{
    process_three_finger_drag, ThreeFingerDragAction, ThreeFingerDragConfig,
    ThreeFingerDragConfigError, ThreeFingerDragDecision, ThreeFingerDragPhase,
    ThreeFingerDragState,
};
pub use time::Monotonic;
pub use units::{LogicalPixels, LogicalPixelsPerMm, Millimeters, NonFiniteError, RawAxis};
