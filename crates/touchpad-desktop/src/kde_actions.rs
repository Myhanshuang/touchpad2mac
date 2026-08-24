//! M15 configurable KDE desktop-action mapping boundary.
//!
//! This module intentionally contains the KDE-facing action identifiers so
//! `touchpad-core` remains desktop-neutral. A real transport is a separately
//! qualified adapter; tests use an injected fake transport only.

#![forbid(unsafe_code)]

use std::collections::HashSet;

use touchpad_core::{
    DesktopAction, GestureMapConfig, GestureTarget, GestureTrigger, OutputError,
    ALL_GESTURE_TRIGGERS,
};
use zbus::blocking::{Connection, Proxy};

const KGLOBALACCEL_BUS: &str = "org.kde.kglobalaccel";
const KGLOBALACCEL_IFACE: &str = "org.kde.kglobalaccel.Component";
const KWIN_BUS: &str = "org.kde.KWin";
const KWIN_EFFECTS_OBJECT: &str = "/Effects";
const KWIN_EFFECTS_IFACE: &str = "org.kde.kwin.Effects";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct KGlobalAccelTarget {
    object_path: &'static str,
    action_id: &'static str,
}

fn target_for_binding(binding: &str) -> Option<KGlobalAccelTarget> {
    match binding {
        "overview" => Some(KGlobalAccelTarget {
            object_path: "/component/kwin",
            action_id: "Overview",
        }),
        "overview-close" => Some(KGlobalAccelTarget {
            object_path: "/component/kwin",
            action_id: "Overview",
        }),
        "present-windows" => Some(KGlobalAccelTarget {
            object_path: "/component/kwin",
            action_id: "Expose",
        }),
        "workspace-next" => Some(KGlobalAccelTarget {
            object_path: "/component/kwin",
            action_id: "Switch to Next Desktop",
        }),
        "workspace-previous" => Some(KGlobalAccelTarget {
            object_path: "/component/kwin",
            action_id: "Switch to Previous Desktop",
        }),
        "show-desktop" => Some(KGlobalAccelTarget {
            object_path: "/component/kwin",
            action_id: "Show Desktop",
        }),
        "application-launcher" => Some(KGlobalAccelTarget {
            object_path: "/component/plasmashell",
            action_id: "activate application launcher",
        }),
        _ => None,
    }
}

/// Whether the production KDE 6 transport has a real desktop action target
/// for this semantic action.
#[must_use]
pub fn real_kde_action_supported(action: DesktopAction) -> bool {
    KdeActionMap::default()
        .binding(action)
        .and_then(target_for_binding)
        .is_some()
}

/// Validates an M18/M19 gesture map against the real KDE output surface.
/// Continuous-gesture passthrough remains unsupported by the M6 portal/libei
/// pointer device and is rejected before grab instead of failing mid-session.
pub fn required_real_kde_actions(
    gestures: &GestureMapConfig,
) -> Result<Vec<DesktopAction>, OutputError> {
    gestures.validate().map_err(|error| {
        OutputError::Unavailable(format!("invalid gesture map for KDE output: {error}"))
    })?;

    let mut required = Vec::new();
    for trigger in ALL_GESTURE_TRIGGERS {
        let target = gestures.target(*trigger);
        let action = match target {
            GestureTarget::Disabled => continue,
            GestureTarget::Passthrough => {
                if *trigger == GestureTrigger::ThreeFingerTap {
                    DesktopAction::Lookup
                } else {
                    return Err(OutputError::Unavailable(format!(
                        "gesture {} is configured as passthrough, but the real KDE M19 output \
                         has no native ContinuousGesture transport; map it to a supported \
                         desktop action or disable it",
                        trigger.name()
                    )));
                }
            }
            other => other.desktop_action().ok_or_else(|| {
                OutputError::Unavailable(format!(
                    "gesture {} has no executable KDE target",
                    trigger.name()
                ))
            })?,
        };

        if !real_kde_action_supported(action) {
            return Err(OutputError::Unavailable(format!(
                "gesture {} maps to {action:?}, which has no real KDE action transport; \
                 supported actions are workspace next/previous, overview, present windows, \
                 show desktop, and application launcher",
                trigger.name()
            )));
        }
        if !required.contains(&action) {
            required.push(action);
        }
    }
    Ok(required)
}

/// One discoverable action mapping. `binding=None` disables the action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KdeActionBinding {
    /// Platform-neutral semantic action.
    pub action: DesktopAction,
    /// KDE-side configured identifier; `None` means disabled.
    pub binding: Option<String>,
}

/// Configurable set of KDE action bindings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KdeActionMap {
    bindings: Vec<KdeActionBinding>,
}

impl Default for KdeActionMap {
    fn default() -> Self {
        let defaults = [
            (DesktopAction::OpenOverview, "overview"),
            (DesktopAction::PresentWindows, "present-windows"),
            (DesktopAction::NextWorkspace, "workspace-next"),
            (DesktopAction::PreviousWorkspace, "workspace-previous"),
            (DesktopAction::ShowDesktop, "show-desktop"),
            (DesktopAction::ApplicationLauncher, "application-launcher"),
            (DesktopAction::CloseOverview, "overview-close"),
            (DesktopAction::NotificationCenter, "notification-center"),
            (DesktopAction::PageNext, "page-next"),
            (DesktopAction::PagePrevious, "page-previous"),
            (DesktopAction::SmartZoom, "smart-zoom"),
            (DesktopAction::Lookup, "lookup"),
        ];
        Self {
            bindings: defaults
                .into_iter()
                .map(|(action, binding)| KdeActionBinding {
                    action,
                    binding: Some(binding.to_string()),
                })
                .collect(),
        }
    }
}

impl KdeActionMap {
    /// All mappings in deterministic discovery order.
    #[must_use]
    pub fn bindings(&self) -> &[KdeActionBinding] {
        &self.bindings
    }

    /// Returns the enabled binding for an action.
    #[must_use]
    pub fn binding(&self, action: DesktopAction) -> Option<&str> {
        self.bindings
            .iter()
            .find(|entry| entry.action == action)
            .and_then(|entry| entry.binding.as_deref())
    }

    /// Enables/remaps (`Some`) or disables (`None`) one semantic action.
    pub fn set_binding(&mut self, action: DesktopAction, binding: Option<String>) {
        if let Some(entry) = self
            .bindings
            .iter_mut()
            .find(|entry| entry.action == action)
        {
            entry.binding = binding;
        } else {
            self.bindings.push(KdeActionBinding { action, binding });
        }
    }
}

/// Injected KDE action transport. A production KWin/shortcut transport must
/// implement this only after its own capability/authorization qualification.
pub trait KdeActionTransport {
    /// Read-only capability check for all bindings this session may invoke.
    /// Fake transports keep the default no-op; the real KDE transport
    /// verifies KGlobalAccel registration without triggering any action.
    fn preflight(&mut self, _bindings: &[&str]) -> Result<(), OutputError> {
        Ok(())
    }

    /// Invokes one already-resolved KDE binding identifier.
    fn invoke(&mut self, binding: &str) -> Result<(), OutputError>;
}

impl<T: KdeActionTransport + ?Sized> KdeActionTransport for Box<T> {
    fn preflight(&mut self, bindings: &[&str]) -> Result<(), OutputError> {
        (**self).preflight(bindings)
    }

    fn invoke(&mut self, binding: &str) -> Result<(), OutputError> {
        (**self).invoke(binding)
    }
}

/// Real KDE Plasma 6 desktop-action transport backed by the session-bus
/// KGlobalAccel component API. Construction is side-effect-free; D-Bus is
/// connected lazily by `preflight`/`invoke` after device qualification.
/// `preflight` only reads registered shortcut names and never triggers an
/// action.
#[derive(Default)]
pub struct KGlobalAccelTransport {
    connection: Option<Connection>,
    verified: HashSet<String>,
}

impl std::fmt::Debug for KGlobalAccelTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KGlobalAccelTransport")
            .field("connected", &self.connection.is_some())
            .field("verified", &self.verified)
            .finish()
    }
}

impl KGlobalAccelTransport {
    /// Creates a disconnected real KGlobalAccel transport. Connecting to the
    /// session bus remains lazy until preflight/invocation.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn connection(&mut self) -> Result<&Connection, OutputError> {
        if self.connection.is_none() {
            let connection = Connection::session().map_err(|error| {
                OutputError::Unavailable(format!(
                    "KDE KGlobalAccel session bus is unavailable: {error}"
                ))
            })?;
            self.connection = Some(connection);
        }
        Ok(self.connection.as_ref().expect("initialized above"))
    }

    fn verify_binding(&mut self, binding: &str) -> Result<KGlobalAccelTarget, OutputError> {
        let target = target_for_binding(binding).ok_or_else(|| {
            OutputError::Unavailable(format!(
                "KDE binding {binding:?} has no real KGlobalAccel target"
            ))
        })?;
        if self.verified.contains(binding) {
            return Ok(target);
        }

        let reply = self
            .connection()?
            .call_method(
                Some(KGLOBALACCEL_BUS),
                target.object_path,
                Some(KGLOBALACCEL_IFACE),
                "shortcutNames",
                &(),
            )
            .map_err(|error| {
                OutputError::Unavailable(format!(
                    "could not query KDE shortcut component {}: {error}",
                    target.object_path
                ))
            })?;
        let names: Vec<String> = reply.body().deserialize().map_err(|error| {
            OutputError::Io(format!(
                "could not decode KDE shortcut names for {}: {error}",
                target.object_path
            ))
        })?;
        if !names.iter().any(|name| name == target.action_id) {
            return Err(OutputError::Unavailable(format!(
                "KDE component {} does not register required action {:?}",
                target.object_path, target.action_id
            )));
        }
        self.verified.insert(binding.to_string());
        Ok(target)
    }

    fn overview_is_active(&mut self) -> Result<bool, OutputError> {
        let proxy = Proxy::new(
            self.connection()?,
            KWIN_BUS,
            KWIN_EFFECTS_OBJECT,
            KWIN_EFFECTS_IFACE,
        )
        .map_err(|error| {
            OutputError::Unavailable(format!(
                "could not create KWin effects proxy for Overview state: {error}"
            ))
        })?;
        let active: Vec<String> = proxy.get_property("activeEffects").map_err(|error| {
            OutputError::Unavailable(format!(
                "could not read KWin activeEffects for Overview state: {error}"
            ))
        })?;
        Ok(active.iter().any(|effect| effect == "overview"))
    }
}

fn overview_toggle_needed(binding: &str, overview_active: bool) -> Option<bool> {
    match binding {
        "overview" => Some(!overview_active),
        "overview-close" => Some(overview_active),
        _ => None,
    }
}

impl KdeActionTransport for KGlobalAccelTransport {
    fn preflight(&mut self, bindings: &[&str]) -> Result<(), OutputError> {
        for binding in bindings {
            self.verify_binding(binding)?;
        }
        if bindings
            .iter()
            .any(|binding| matches!(*binding, "overview" | "overview-close"))
        {
            let _ = self.overview_is_active()?;
        }
        Ok(())
    }

    fn invoke(&mut self, binding: &str) -> Result<(), OutputError> {
        let target = self.verify_binding(binding)?;
        if matches!(binding, "overview" | "overview-close") {
            let needed = overview_toggle_needed(binding, self.overview_is_active()?)
                .expect("overview bindings are handled above");
            if !needed {
                return Ok(());
            }
        }
        self.connection()?
            .call_method(
                Some(KGLOBALACCEL_BUS),
                target.object_path,
                Some(KGLOBALACCEL_IFACE),
                "invokeShortcut",
                &(target.action_id,),
            )
            .map_err(|error| {
                OutputError::Io(format!(
                    "KDE shortcut {:?} on {} failed: {error}",
                    target.action_id, target.object_path
                ))
            })?;
        Ok(())
    }
}

/// Maps platform-neutral desktop actions onto configurable KDE identifiers.
pub struct KdeActionAdapter<T> {
    map: KdeActionMap,
    transport: T,
}

impl<T: KdeActionTransport> KdeActionAdapter<T> {
    /// Creates an adapter with the provided mapping and transport.
    #[must_use]
    pub fn new(map: KdeActionMap, transport: T) -> Self {
        Self { map, transport }
    }

    /// Current discoverable mapping.
    #[must_use]
    pub const fn map(&self) -> &KdeActionMap {
        &self.map
    }

    /// Mutable mapping for explicit runtime/user remapping.
    pub fn map_mut(&mut self) -> &mut KdeActionMap {
        &mut self.map
    }

    /// Triggers an enabled semantic action or returns an honest unavailable
    /// error when the action is disabled/unmapped.
    pub fn trigger(&mut self, action: DesktopAction) -> Result<(), OutputError> {
        let binding = self.map.binding(action).ok_or_else(|| {
            OutputError::Unavailable(format!(
                "KDE desktop action {action:?} is disabled/unmapped"
            ))
        })?;
        self.transport.invoke(binding)
    }

    /// Read-only preflight of exactly the semantic actions this session may
    /// emit. No shortcut is invoked.
    pub fn preflight_actions(&mut self, actions: &[DesktopAction]) -> Result<(), OutputError> {
        let mut bindings = Vec::with_capacity(actions.len());
        for action in actions {
            let binding = self.map.binding(*action).ok_or_else(|| {
                OutputError::Unavailable(format!(
                    "KDE desktop action {action:?} is disabled/unmapped"
                ))
            })?;
            bindings.push(binding);
        }
        self.transport.preflight(&bindings)
    }

    /// Returns the owned parts, useful for deterministic test inspection.
    #[must_use]
    pub fn into_parts(self) -> (KdeActionMap, T) {
        (self.map, self.transport)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeTransport {
        calls: Vec<String>,
        fail: bool,
    }
    impl KdeActionTransport for FakeTransport {
        fn invoke(&mut self, binding: &str) -> Result<(), OutputError> {
            if self.fail {
                Err(OutputError::Io("fake action failure".into()))
            } else {
                self.calls.push(binding.to_string());
                Ok(())
            }
        }
    }

    #[test]
    fn defaults_are_discoverable_and_remappable() {
        let mut map = KdeActionMap::default();
        assert_eq!(map.binding(DesktopAction::OpenOverview), Some("overview"));
        map.set_binding(DesktopAction::OpenOverview, Some("my-overview".into()));
        assert_eq!(
            map.binding(DesktopAction::OpenOverview),
            Some("my-overview")
        );
        map.set_binding(DesktopAction::OpenOverview, None);
        assert_eq!(map.binding(DesktopAction::OpenOverview), None);
    }

    #[test]
    fn adapter_orders_calls_and_propagates_transport_failure() {
        let mut adapter = KdeActionAdapter::new(KdeActionMap::default(), FakeTransport::default());
        adapter.trigger(DesktopAction::NextWorkspace).unwrap();
        adapter.trigger(DesktopAction::ShowDesktop).unwrap();
        let (_, transport) = adapter.into_parts();
        assert_eq!(transport.calls, ["workspace-next", "show-desktop"]);

        let mut failing = KdeActionAdapter::new(
            KdeActionMap::default(),
            FakeTransport {
                calls: vec![],
                fail: true,
            },
        );
        assert!(matches!(
            failing.trigger(DesktopAction::OpenOverview),
            Err(OutputError::Io(_))
        ));
    }

    #[test]
    fn disabled_action_is_honestly_unavailable() {
        let mut map = KdeActionMap::default();
        map.set_binding(DesktopAction::SmartZoom, None);
        let mut adapter = KdeActionAdapter::new(map, FakeTransport::default());
        assert!(matches!(
            adapter.trigger(DesktopAction::SmartZoom),
            Err(OutputError::Unavailable(_))
        ));
    }

    #[test]
    fn real_kde_support_set_is_explicit_and_passthrough_is_rejected() {
        for supported in [
            DesktopAction::NextWorkspace,
            DesktopAction::PreviousWorkspace,
            DesktopAction::ShowDesktop,
            DesktopAction::OpenOverview,
            DesktopAction::CloseOverview,
            DesktopAction::PresentWindows,
            DesktopAction::ApplicationLauncher,
        ] {
            assert!(real_kde_action_supported(supported), "{supported:?}");
        }
        for unsupported in [
            DesktopAction::NotificationCenter,
            DesktopAction::PageNext,
            DesktopAction::PagePrevious,
            DesktopAction::SmartZoom,
            DesktopAction::Lookup,
        ] {
            assert!(!real_kde_action_supported(unsupported), "{unsupported:?}");
        }

        let defaults = GestureMapConfig::default();
        assert!(matches!(
            required_real_kde_actions(&defaults),
            Err(OutputError::Unavailable(_))
        ));

        let macos = GestureMapConfig::macos_inspired();
        assert_eq!(
            required_real_kde_actions(&macos).unwrap(),
            vec![
                DesktopAction::NextWorkspace,
                DesktopAction::PreviousWorkspace,
                DesktopAction::OpenOverview,
                DesktopAction::PresentWindows,
                DesktopAction::ApplicationLauncher,
                DesktopAction::ShowDesktop,
            ]
        );
    }

    #[test]
    fn overview_open_close_are_directional_over_the_toggle_shortcut() {
        assert_eq!(overview_toggle_needed("overview", false), Some(true));
        assert_eq!(overview_toggle_needed("overview", true), Some(false));
        assert_eq!(overview_toggle_needed("overview-close", true), Some(true));
        assert_eq!(overview_toggle_needed("overview-close", false), Some(false));
        assert_eq!(overview_toggle_needed("workspace-next", false), None);
    }
}
