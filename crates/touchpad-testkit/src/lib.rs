//! System-test helpers.
//!
//! Production crates remain fully mockable and do not depend on this crate.
//! On Linux, the optional [`uinput`] module creates a real virtual Type-B
//! touchpad so tests traverse the kernel input subsystem before reaching
//! `touchpad-linux`.

#![warn(missing_docs)]

#[cfg(target_os = "linux")]
pub mod uinput;
