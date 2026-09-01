//! M18 desktop-neutral gesture-to-action routing.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{
    ContinuousGestureEvent, ContinuousGestureKind, ContinuousGesturePhase, DesktopAction,
    MouseButton, OutputEvent,
};

pub const GESTURE_MAP_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GestureTrigger {
    PinchIn,
    PinchOut,
    RotateClockwise,
    RotateCounterClockwise,
    TwoFingerPageLeft,
    TwoFingerPageRight,
    TwoFingerPageUp,
    TwoFingerPageDown,
    ThreeFingerSwipeLeft,
    ThreeFingerSwipeRight,
    ThreeFingerSwipeUp,
    ThreeFingerSwipeDown,
    FourFingerSwipeLeft,
    FourFingerSwipeRight,
    FourFingerSwipeUp,
    FourFingerSwipeDown,
    EdgeSwipeLeft,
    EdgeSwipeRight,
    EdgeSwipeUp,
    EdgeSwipeDown,
    ThumbThreePinch,
    ThumbThreeSpread,
    ThreeFingerTap,
}

impl GestureTrigger {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::PinchIn => "pinch-in",
            Self::PinchOut => "pinch-out",
            Self::RotateClockwise => "rotate-clockwise",
            Self::RotateCounterClockwise => "rotate-counter-clockwise",
            Self::TwoFingerPageLeft => "two-finger-page-left",
            Self::TwoFingerPageRight => "two-finger-page-right",
            Self::TwoFingerPageUp => "two-finger-page-up",
            Self::TwoFingerPageDown => "two-finger-page-down",
            Self::ThreeFingerSwipeLeft => "three-finger-swipe-left",
            Self::ThreeFingerSwipeRight => "three-finger-swipe-right",
            Self::ThreeFingerSwipeUp => "three-finger-swipe-up",
            Self::ThreeFingerSwipeDown => "three-finger-swipe-down",
            Self::FourFingerSwipeLeft => "four-finger-swipe-left",
            Self::FourFingerSwipeRight => "four-finger-swipe-right",
            Self::FourFingerSwipeUp => "four-finger-swipe-up",
            Self::FourFingerSwipeDown => "four-finger-swipe-down",
            Self::EdgeSwipeLeft => "edge-swipe-left",
            Self::EdgeSwipeRight => "edge-swipe-right",
            Self::EdgeSwipeUp => "edge-swipe-up",
            Self::EdgeSwipeDown => "edge-swipe-down",
            Self::ThumbThreePinch => "thumb-three-pinch",
            Self::ThumbThreeSpread => "thumb-three-spread",
            Self::ThreeFingerTap => "three-finger-tap",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        ALL_GESTURE_TRIGGERS
            .iter()
            .copied()
            .find(|trigger| trigger.name() == value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GestureTarget {
    Passthrough,
    Disabled,
    MiddleClick,
    NextWorkspace,
    PreviousWorkspace,
    ShowDesktop,
    OpenOverview,
    CloseOverview,
    PresentWindows,
    ApplicationLauncher,
    NotificationCenter,
    PageNext,
    PagePrevious,
    SmartZoom,
    Lookup,
}

impl GestureTarget {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Passthrough => "passthrough",
            Self::Disabled => "disabled",
            Self::MiddleClick => "middle-click",
            Self::NextWorkspace => "next-workspace",
            Self::PreviousWorkspace => "previous-workspace",
            Self::ShowDesktop => "show-desktop",
            Self::OpenOverview => "open-overview",
            Self::CloseOverview => "close-overview",
            Self::PresentWindows => "present-windows",
            Self::ApplicationLauncher => "application-launcher",
            Self::NotificationCenter => "notification-center",
            Self::PageNext => "page-next",
            Self::PagePrevious => "page-previous",
            Self::SmartZoom => "smart-zoom",
            Self::Lookup => "lookup",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "passthrough" => Some(Self::Passthrough),
            "disabled" => Some(Self::Disabled),
            "middle-click" => Some(Self::MiddleClick),
            "next-workspace" => Some(Self::NextWorkspace),
            "previous-workspace" => Some(Self::PreviousWorkspace),
            "show-desktop" => Some(Self::ShowDesktop),
            "open-overview" => Some(Self::OpenOverview),
            "close-overview" => Some(Self::CloseOverview),
            "present-windows" => Some(Self::PresentWindows),
            "application-launcher" => Some(Self::ApplicationLauncher),
            "notification-center" => Some(Self::NotificationCenter),
            "page-next" => Some(Self::PageNext),
            "page-previous" => Some(Self::PagePrevious),
            "smart-zoom" => Some(Self::SmartZoom),
            "lookup" => Some(Self::Lookup),
            _ => None,
        }
    }

    #[must_use]
    pub const fn desktop_action(self) -> Option<DesktopAction> {
        match self {
            Self::Passthrough | Self::Disabled | Self::MiddleClick => None,
            Self::NextWorkspace => Some(DesktopAction::NextWorkspace),
            Self::PreviousWorkspace => Some(DesktopAction::PreviousWorkspace),
            Self::ShowDesktop => Some(DesktopAction::ShowDesktop),
            Self::OpenOverview => Some(DesktopAction::OpenOverview),
            Self::CloseOverview => Some(DesktopAction::CloseOverview),
            Self::PresentWindows => Some(DesktopAction::PresentWindows),
            Self::ApplicationLauncher => Some(DesktopAction::ApplicationLauncher),
            Self::NotificationCenter => Some(DesktopAction::NotificationCenter),
            Self::PageNext => Some(DesktopAction::PageNext),
            Self::PagePrevious => Some(DesktopAction::PagePrevious),
            Self::SmartZoom => Some(DesktopAction::SmartZoom),
            Self::Lookup => Some(DesktopAction::Lookup),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GestureMapConfig {
    pub version: u32,
    /// Whether M15 three-finger drag may commit before M14 three-finger swipe.
    /// Disabling drag keeps three-finger tap recognition but lets swipe
    /// mappings become reachable.
    pub three_finger_drag_enabled: bool,
    pub bindings: BTreeMap<GestureTrigger, GestureTarget>,
}

impl Default for GestureMapConfig {
    fn default() -> Self {
        let mut bindings = BTreeMap::new();
        for trigger in ALL_GESTURE_TRIGGERS {
            bindings.insert(*trigger, GestureTarget::Passthrough);
        }
        bindings.insert(GestureTrigger::ThreeFingerTap, GestureTarget::MiddleClick);
        Self {
            version: GESTURE_MAP_VERSION,
            three_finger_drag_enabled: true,
            bindings,
        }
    }
}

impl GestureMapConfig {
    pub fn validate(&self) -> Result<(), GestureMapError> {
        if self.version != GESTURE_MAP_VERSION {
            return Err(GestureMapError::UnsupportedVersion(self.version));
        }
        for trigger in ALL_GESTURE_TRIGGERS {
            if !self.bindings.contains_key(trigger) {
                return Err(GestureMapError::MissingTrigger(*trigger));
            }
        }
        if self.bindings.len() != ALL_GESTURE_TRIGGERS.len() {
            return Err(GestureMapError::UnexpectedBindingCount(self.bindings.len()));
        }
        for (trigger, target) in &self.bindings {
            if *target == GestureTarget::MiddleClick && *trigger != GestureTrigger::ThreeFingerTap {
                return Err(GestureMapError::MiddleClickOnlyThreeFingerTap(*trigger));
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn target(&self, trigger: GestureTrigger) -> GestureTarget {
        self.bindings
            .get(&trigger)
            .copied()
            .unwrap_or(GestureTarget::Passthrough)
    }

    pub fn set_target(
        &mut self,
        trigger: GestureTrigger,
        target: GestureTarget,
    ) -> Result<(), GestureMapError> {
        let previous = self.bindings.insert(trigger, target);
        if let Err(error) = self.validate() {
            match previous {
                Some(previous) => {
                    self.bindings.insert(trigger, previous);
                }
                None => {
                    self.bindings.remove(&trigger);
                }
            }
            return Err(error);
        }
        Ok(())
    }

    #[must_use]
    pub fn macos_inspired() -> Self {
        let mut config = Self {
            three_finger_drag_enabled: false,
            ..Self::default()
        };
        for trigger in ALL_GESTURE_TRIGGERS {
            config.bindings.insert(*trigger, GestureTarget::Disabled);
        }
        for (trigger, target) in [
            (
                GestureTrigger::ThreeFingerSwipeLeft,
                GestureTarget::NextWorkspace,
            ),
            (
                GestureTrigger::ThreeFingerSwipeRight,
                GestureTarget::PreviousWorkspace,
            ),
            (
                GestureTrigger::ThreeFingerSwipeUp,
                GestureTarget::OpenOverview,
            ),
            (
                GestureTrigger::ThreeFingerSwipeDown,
                GestureTarget::PresentWindows,
            ),
            (
                GestureTrigger::ThumbThreePinch,
                GestureTarget::ApplicationLauncher,
            ),
            (GestureTrigger::ThumbThreeSpread, GestureTarget::ShowDesktop),
        ] {
            config.bindings.insert(trigger, target);
        }
        config
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GestureMapError {
    #[error("unsupported gesture-map version {0}")]
    UnsupportedVersion(u32),
    #[error("gesture map is missing trigger {0:?}")]
    MissingTrigger(GestureTrigger),
    #[error("gesture map has unexpected binding count {0}")]
    UnexpectedBindingCount(usize),
    #[error("middle-click is only valid for three-finger-tap, not {0:?}")]
    MiddleClickOnlyThreeFingerTap(GestureTrigger),
}

pub const ALL_GESTURE_TRIGGERS: &[GestureTrigger] = &[
    GestureTrigger::PinchIn,
    GestureTrigger::PinchOut,
    GestureTrigger::RotateClockwise,
    GestureTrigger::RotateCounterClockwise,
    GestureTrigger::TwoFingerPageLeft,
    GestureTrigger::TwoFingerPageRight,
    GestureTrigger::TwoFingerPageUp,
    GestureTrigger::TwoFingerPageDown,
    GestureTrigger::ThreeFingerSwipeLeft,
    GestureTrigger::ThreeFingerSwipeRight,
    GestureTrigger::ThreeFingerSwipeUp,
    GestureTrigger::ThreeFingerSwipeDown,
    GestureTrigger::FourFingerSwipeLeft,
    GestureTrigger::FourFingerSwipeRight,
    GestureTrigger::FourFingerSwipeUp,
    GestureTrigger::FourFingerSwipeDown,
    GestureTrigger::EdgeSwipeLeft,
    GestureTrigger::EdgeSwipeRight,
    GestureTrigger::EdgeSwipeUp,
    GestureTrigger::EdgeSwipeDown,
    GestureTrigger::ThumbThreePinch,
    GestureTrigger::ThumbThreeSpread,
    GestureTrigger::ThreeFingerTap,
];

pub const ALL_GESTURE_TARGETS: &[GestureTarget] = &[
    GestureTarget::Passthrough,
    GestureTarget::Disabled,
    GestureTarget::MiddleClick,
    GestureTarget::NextWorkspace,
    GestureTarget::PreviousWorkspace,
    GestureTarget::ShowDesktop,
    GestureTarget::OpenOverview,
    GestureTarget::CloseOverview,
    GestureTarget::PresentWindows,
    GestureTarget::ApplicationLauncher,
    GestureTarget::NotificationCenter,
    GestureTarget::PageNext,
    GestureTarget::PagePrevious,
    GestureTarget::SmartZoom,
    GestureTarget::Lookup,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActiveRoute {
    Passthrough(ContinuousGestureKind),
    Suppressed(ContinuousGestureKind),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GestureRouteState {
    active: Option<ActiveRoute>,
}

impl GestureRouteState {
    pub fn reset(&mut self) {
        self.active = None;
    }
}

#[must_use]
pub fn route_continuous_gesture(
    config: &GestureMapConfig,
    state: &mut GestureRouteState,
    event: ContinuousGestureEvent,
) -> Option<OutputEvent> {
    match event.phase {
        ContinuousGesturePhase::Begin => {
            let Some(trigger) = trigger_from_begin(&event) else {
                state.active = Some(ActiveRoute::Passthrough(event.kind));
                return Some(OutputEvent::ContinuousGesture(event));
            };
            match config.target(trigger) {
                GestureTarget::Passthrough => {
                    state.active = Some(ActiveRoute::Passthrough(event.kind));
                    Some(OutputEvent::ContinuousGesture(event))
                }
                GestureTarget::Disabled => {
                    state.active = Some(ActiveRoute::Suppressed(event.kind));
                    None
                }
                target => {
                    state.active = Some(ActiveRoute::Suppressed(event.kind));
                    target.desktop_action().map(OutputEvent::DesktopAction)
                }
            }
        }
        ContinuousGesturePhase::Update => match state.active {
            Some(ActiveRoute::Passthrough(kind)) if kind == event.kind => {
                Some(OutputEvent::ContinuousGesture(event))
            }
            Some(ActiveRoute::Suppressed(kind)) if kind == event.kind => None,
            _ => Some(OutputEvent::ContinuousGesture(event)),
        },
        ContinuousGesturePhase::End | ContinuousGesturePhase::Cancel => {
            let result = match state.active {
                Some(ActiveRoute::Passthrough(kind)) if kind == event.kind => {
                    Some(OutputEvent::ContinuousGesture(event))
                }
                Some(ActiveRoute::Suppressed(kind)) if kind == event.kind => None,
                _ => Some(OutputEvent::ContinuousGesture(event)),
            };
            state.reset();
            result
        }
    }
}

#[must_use]
pub fn route_three_finger_tap(config: &GestureMapConfig) -> Vec<OutputEvent> {
    match config.target(GestureTrigger::ThreeFingerTap) {
        GestureTarget::Disabled => Vec::new(),
        GestureTarget::MiddleClick => vec![
            OutputEvent::ButtonDown(MouseButton::Middle),
            OutputEvent::ButtonUp(MouseButton::Middle),
        ],
        GestureTarget::Passthrough => {
            vec![OutputEvent::DesktopAction(DesktopAction::Lookup)]
        }
        target => target
            .desktop_action()
            .map(OutputEvent::DesktopAction)
            .into_iter()
            .collect(),
    }
}

fn trigger_from_begin(event: &ContinuousGestureEvent) -> Option<GestureTrigger> {
    match event.kind {
        ContinuousGestureKind::Pinch => {
            if event.scale < 1.0 {
                Some(GestureTrigger::PinchIn)
            } else if event.scale > 1.0 {
                Some(GestureTrigger::PinchOut)
            } else {
                None
            }
        }
        ContinuousGestureKind::Rotate => {
            if event.rotation_radians < 0.0 {
                Some(GestureTrigger::RotateClockwise)
            } else if event.rotation_radians > 0.0 {
                Some(GestureTrigger::RotateCounterClockwise)
            } else {
                None
            }
        }
        ContinuousGestureKind::TwoFingerPageSwipe => direction_trigger(
            event.translation_x_mm,
            event.translation_y_mm,
            GestureTrigger::TwoFingerPageLeft,
            GestureTrigger::TwoFingerPageRight,
            GestureTrigger::TwoFingerPageUp,
            GestureTrigger::TwoFingerPageDown,
        ),
        ContinuousGestureKind::ThreeFingerSwipe => direction_trigger(
            event.translation_x_mm,
            event.translation_y_mm,
            GestureTrigger::ThreeFingerSwipeLeft,
            GestureTrigger::ThreeFingerSwipeRight,
            GestureTrigger::ThreeFingerSwipeUp,
            GestureTrigger::ThreeFingerSwipeDown,
        ),
        ContinuousGestureKind::FourFingerSwipe => direction_trigger(
            event.translation_x_mm,
            event.translation_y_mm,
            GestureTrigger::FourFingerSwipeLeft,
            GestureTrigger::FourFingerSwipeRight,
            GestureTrigger::FourFingerSwipeUp,
            GestureTrigger::FourFingerSwipeDown,
        ),
        ContinuousGestureKind::EdgeSwipe => direction_trigger(
            event.translation_x_mm,
            event.translation_y_mm,
            GestureTrigger::EdgeSwipeLeft,
            GestureTrigger::EdgeSwipeRight,
            GestureTrigger::EdgeSwipeUp,
            GestureTrigger::EdgeSwipeDown,
        ),
        ContinuousGestureKind::ThumbThreePinch => Some(GestureTrigger::ThumbThreePinch),
        ContinuousGestureKind::ThumbThreeSpread => Some(GestureTrigger::ThumbThreeSpread),
    }
}

fn direction_trigger(
    x: f32,
    y: f32,
    left: GestureTrigger,
    right: GestureTrigger,
    up: GestureTrigger,
    down: GestureTrigger,
) -> Option<GestureTrigger> {
    if x.abs() >= y.abs() && x != 0.0 {
        Some(if x < 0.0 { left } else { right })
    } else if y != 0.0 {
        Some(if y < 0.0 { up } else { down })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(
        kind: ContinuousGestureKind,
        phase: ContinuousGesturePhase,
        x: f32,
        y: f32,
    ) -> ContinuousGestureEvent {
        ContinuousGestureEvent {
            kind,
            phase,
            translation_x_mm: x,
            translation_y_mm: y,
            scale: 1.0,
            rotation_radians: 0.0,
        }
    }

    #[test]
    fn default_is_complete_and_maps_three_finger_tap_to_middle_click() {
        let config = GestureMapConfig::default();
        config.validate().unwrap();
        assert_eq!(config.bindings.len(), ALL_GESTURE_TRIGGERS.len());
        assert_eq!(
            config.target(GestureTrigger::ThreeFingerTap),
            GestureTarget::MiddleClick
        );
        assert_eq!(
            route_three_finger_tap(&config),
            vec![
                OutputEvent::ButtonDown(MouseButton::Middle),
                OutputEvent::ButtonUp(MouseButton::Middle),
            ]
        );
        assert_eq!(
            config.target(GestureTrigger::PinchIn),
            GestureTarget::Passthrough
        );
    }

    #[test]
    fn middle_click_target_is_restricted_to_three_finger_tap_atomically() {
        let mut config = GestureMapConfig::default();
        let before = config.clone();
        assert!(matches!(
            config.set_target(GestureTrigger::PinchIn, GestureTarget::MiddleClick),
            Err(GestureMapError::MiddleClickOnlyThreeFingerTap(
                GestureTrigger::PinchIn
            ))
        ));
        assert_eq!(config, before);
    }

    #[test]
    fn mapped_swipe_fires_once_and_suppresses_updates_and_end() {
        let mut config = GestureMapConfig::default();
        config
            .set_target(
                GestureTrigger::ThreeFingerSwipeUp,
                GestureTarget::OpenOverview,
            )
            .unwrap();
        let mut state = GestureRouteState::default();
        assert_eq!(
            route_continuous_gesture(
                &config,
                &mut state,
                event(
                    ContinuousGestureKind::ThreeFingerSwipe,
                    ContinuousGesturePhase::Begin,
                    0.0,
                    -2.0,
                ),
            ),
            Some(OutputEvent::DesktopAction(DesktopAction::OpenOverview))
        );
        assert_eq!(
            route_continuous_gesture(
                &config,
                &mut state,
                event(
                    ContinuousGestureKind::ThreeFingerSwipe,
                    ContinuousGesturePhase::Update,
                    0.0,
                    -4.0,
                ),
            ),
            None
        );
        assert_eq!(
            route_continuous_gesture(
                &config,
                &mut state,
                event(
                    ContinuousGestureKind::ThreeFingerSwipe,
                    ContinuousGesturePhase::End,
                    0.0,
                    0.0,
                ),
            ),
            None
        );
    }

    #[test]
    fn macos_inspired_preset_has_expected_assignments() {
        let config = GestureMapConfig::macos_inspired();
        assert!(!config.three_finger_drag_enabled);
        assert_eq!(
            config.target(GestureTrigger::ThreeFingerSwipeUp),
            GestureTarget::OpenOverview
        );
        assert_eq!(
            config.target(GestureTrigger::ThumbThreeSpread),
            GestureTarget::ShowDesktop
        );
        assert_eq!(
            config.target(GestureTrigger::PinchIn),
            GestureTarget::Disabled
        );
        assert_eq!(
            config.target(GestureTrigger::ThreeFingerTap),
            GestureTarget::Disabled
        );
    }
}
