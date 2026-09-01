#![forbid(unsafe_code)]
//! Negotiated output capabilities (M6).
//!
//! The adapter translates the typed [`touchpad_core::OutputEvent`] contract
//! for relative pointer motion, primary/secondary buttons, and pixel-precise
//! smooth scroll **only when the negotiated device exposes those
//! capabilities** — support is never assumed from the API name alone
//! (PHASE2_PLAN.md §2: the compositor/EIS implementation does the final
//! processing, so every capability claim is subject to the manual A/B
//! measurement before the backend can be marked qualified).

/// A single output capability the adapter may or may not be able to provide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    /// Relative pointer motion (`OutputEvent::PointerMove`).
    RelativePointer,
    /// Primary button press/release (left button).
    PrimaryButton,
    /// Secondary button press/release (right button).
    SecondaryButton,
    /// Middle button press/release.
    MiddleButton,
    /// Pixel-precise smooth scroll lifecycle
    /// (`ScrollBegin`/`ScrollDelta`/`ScrollEnd`).
    PixelScroll,
}

/// The set of output capabilities a negotiated libei device exposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OutputCapabilities {
    /// Relative pointer motion is available (`EI_DEVICE_CAP_POINTER`).
    pub relative_pointer: bool,
    /// Button events are available (`EI_DEVICE_CAP_BUTTON`); the primary
    /// code is `BTN_LEFT` (0x110) per `linux/input-event-codes.h`.
    pub primary_button: bool,
    /// Button events are available; the secondary code is `BTN_RIGHT`
    /// (0x111). (libei 1.6 exposes buttons as one capability; whether the
    /// compositor actually honors `BTN_RIGHT` is verified by the manual A/B
    /// measurement.)
    pub secondary_button: bool,
    /// Button events are available; the middle code is `BTN_MIDDLE`.
    pub middle_button: bool,
    /// Pixel-precise scroll is available (`EI_DEVICE_CAP_SCROLL`).
    pub pixel_scroll: bool,
}

/// Raw libei device-capability bits (`enum ei_device_capability` in
/// `libei.h` 1.6): `POINTER = 1 << 0`, `POINTER_ABSOLUTE = 1 << 1`,
/// `KEYBOARD = 1 << 2`, `TOUCH = 1 << 3`, `SCROLL = 1 << 4`,
/// `BUTTON = 1 << 5`.
pub mod libei_capability_bits {
    /// `EI_DEVICE_CAP_POINTER`.
    pub const POINTER: u32 = 1 << 0;
    /// `EI_DEVICE_CAP_POINTER_ABSOLUTE`.
    pub const POINTER_ABSOLUTE: u32 = 1 << 1;
    /// `EI_DEVICE_CAP_KEYBOARD`.
    pub const KEYBOARD: u32 = 1 << 2;
    /// `EI_DEVICE_CAP_TOUCH`.
    pub const TOUCH: u32 = 1 << 3;
    /// `EI_DEVICE_CAP_SCROLL`.
    pub const SCROLL: u32 = 1 << 4;
    /// `EI_DEVICE_CAP_BUTTON`.
    pub const BUTTON: u32 = 1 << 5;
}

impl OutputCapabilities {
    /// A device exposing no useful output capability.
    pub const NONE: Self = Self {
        relative_pointer: false,
        primary_button: false,
        secondary_button: false,
        middle_button: false,
        pixel_scroll: false,
    };

    /// Derives the output capabilities from raw libei device capability
    /// bits. Relative pointer, buttons, and pixel scroll are reported only
    /// when the corresponding bits are present. The adapter never binds or
    /// exposes touch/contacts: [`libei_capability_bits::TOUCH`] is
    /// deliberately not mapped (M6 safety constraint: no virtual touchpad,
    /// no raw finger count to the compositor).
    #[must_use]
    pub fn from_device_capability_bits(bits: u32) -> Self {
        let buttons = bits & libei_capability_bits::BUTTON != 0;
        Self {
            relative_pointer: bits & libei_capability_bits::POINTER != 0,
            primary_button: buttons,
            secondary_button: buttons,
            middle_button: buttons,
            pixel_scroll: bits & libei_capability_bits::SCROLL != 0,
        }
    }

    /// Whether the given capability is available.
    #[must_use]
    pub fn supports(&self, capability: Capability) -> bool {
        match capability {
            Capability::RelativePointer => self.relative_pointer,
            Capability::PrimaryButton => self.primary_button,
            Capability::SecondaryButton => self.secondary_button,
            Capability::MiddleButton => self.middle_button,
            Capability::PixelScroll => self.pixel_scroll,
        }
    }

    /// Whether any capability at all is available.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !self.relative_pointer
            && !self.primary_button
            && !self.secondary_button
            && !self.middle_button
            && !self.pixel_scroll
    }

    /// A stable human-readable summary (also used by `output-probe`).
    #[must_use]
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.relative_pointer {
            parts.push("relative pointer");
        }
        if self.primary_button {
            parts.push("primary button");
        }
        if self.secondary_button {
            parts.push("secondary button");
        }
        if self.middle_button {
            parts.push("middle button");
        }
        if self.pixel_scroll {
            parts.push("pixel-precise smooth scroll");
        }
        if parts.is_empty() {
            "none".to_string()
        } else {
            parts.join(", ")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_libei_bits_honestly() {
        // Pointer + button + scroll -> everything the M6 contract needs.
        let full = OutputCapabilities::from_device_capability_bits(
            libei_capability_bits::POINTER
                | libei_capability_bits::BUTTON
                | libei_capability_bits::SCROLL,
        );
        assert!(full.supports(Capability::RelativePointer));
        assert!(full.supports(Capability::PrimaryButton));
        assert!(full.supports(Capability::SecondaryButton));
        assert!(full.supports(Capability::MiddleButton));
        assert!(full.supports(Capability::PixelScroll));

        // A pointer-only device cannot scroll or click.
        let pointer_only =
            OutputCapabilities::from_device_capability_bits(libei_capability_bits::POINTER);
        assert!(pointer_only.supports(Capability::RelativePointer));
        assert!(!pointer_only.supports(Capability::PrimaryButton));
        assert!(!pointer_only.supports(Capability::SecondaryButton));
        assert!(!pointer_only.supports(Capability::MiddleButton));
        assert!(!pointer_only.supports(Capability::PixelScroll));

        // A scroll-only device cannot move the pointer.
        let scroll_only =
            OutputCapabilities::from_device_capability_bits(libei_capability_bits::SCROLL);
        assert!(!scroll_only.supports(Capability::RelativePointer));
        assert!(scroll_only.supports(Capability::PixelScroll));

        // Touch is deliberately never mapped (no virtual touchpad).
        let touch_only =
            OutputCapabilities::from_device_capability_bits(libei_capability_bits::TOUCH);
        assert!(touch_only.is_empty());
    }

    #[test]
    fn none_is_empty_and_summary_is_truthful() {
        assert!(OutputCapabilities::NONE.is_empty());
        assert_eq!(OutputCapabilities::NONE.summary(), "none");
        let full = OutputCapabilities::from_device_capability_bits(
            libei_capability_bits::POINTER
                | libei_capability_bits::BUTTON
                | libei_capability_bits::SCROLL,
        );
        let summary = full.summary();
        assert!(summary.contains("relative pointer"), "{summary}");
        assert!(summary.contains("primary button"), "{summary}");
        assert!(summary.contains("secondary button"), "{summary}");
        assert!(summary.contains("middle button"), "{summary}");
        assert!(summary.contains("pixel-precise smooth scroll"), "{summary}");
    }
}
