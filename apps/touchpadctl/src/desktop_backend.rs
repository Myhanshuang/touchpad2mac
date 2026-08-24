//! Application-side desktop backend composition.
//!
//! Core profiles decide interaction semantics. This module decides how those
//! semantic outputs are delivered on the selected desktop environment.

#![forbid(unsafe_code)]

use touchpad_core::{DesktopAction, UserSettings};
use touchpad_desktop::{
    required_real_kde_actions, DesktopOutputError, RealKdeStreamingOutputFactory,
    RealStreamingOutputFactory, StreamingOutput, StreamingOutputFactory,
};

use crate::env::RealDesktopBackend;

/// Prepared application-side plan for the real desktop output backend.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RealDesktopPlan {
    /// XDG RemoteDesktop portal + libei only.
    PortalLibei,
    /// Portal/libei plus KDE KGlobalAccel actions.
    KdeComposite {
        /// Semantic desktop actions required by the loaded settings.
        required_actions: Vec<DesktopAction>,
    },
}

impl RealDesktopPlan {
    /// Builds a real-backend plan independently from the selected core
    /// interaction profile.
    pub fn build(
        backend: RealDesktopBackend,
        settings: Option<&UserSettings>,
    ) -> Result<Self, touchpad_core::OutputError> {
        match backend {
            RealDesktopBackend::PortalLibei => Ok(Self::PortalLibei),
            RealDesktopBackend::KdeComposite => {
                let required_actions = match settings {
                    Some(settings) => required_real_kde_actions(&settings.gestures)?,
                    None => Vec::new(),
                };
                Ok(Self::KdeComposite { required_actions })
            }
        }
    }

    /// Creates the concrete streaming session selected by this plan.
    pub fn create_output(&self) -> Result<Box<dyn StreamingOutput>, DesktopOutputError> {
        match self {
            Self::PortalLibei => {
                let mut factory = RealStreamingOutputFactory;
                factory.create()
            }
            Self::KdeComposite { required_actions } => {
                let mut factory = RealKdeStreamingOutputFactory::new(required_actions.clone());
                factory.create()
            }
        }
    }

    /// Validates a hot-reloaded settings document against the active real
    /// backend without changing the running session.
    pub fn validate_reload(&self, settings: &UserSettings) -> Result<(), String> {
        match self {
            Self::PortalLibei => Ok(()),
            Self::KdeComposite { .. } => required_real_kde_actions(&settings.gestures)
                .map(|_| ())
                .map_err(|error| error.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_selection_is_independent_from_profile_identity() {
        let portal = RealDesktopPlan::build(RealDesktopBackend::PortalLibei, None).unwrap();
        let kde = RealDesktopPlan::build(RealDesktopBackend::KdeComposite, None).unwrap();
        assert_eq!(portal, RealDesktopPlan::PortalLibei);
        assert_eq!(
            kde,
            RealDesktopPlan::KdeComposite {
                required_actions: Vec::new()
            }
        );
    }
}
