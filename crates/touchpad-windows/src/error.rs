//! Windows platform errors.

#![forbid(unsafe_code)]

/// Failure of the Windows platform adapter.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WindowsError {
    /// A Windows-only operation was requested on another platform.
    #[error("Windows backend requested on non-Windows host")]
    NotWindows,
    /// A Win32 call failed. The operation and OS error text are preserved.
    #[error("Win32 operation {operation} failed: {message}")]
    Win32 {
        /// API or operation name.
        operation: &'static str,
        /// `std::io::Error::last_os_error()` rendered at the failure point.
        message: String,
    },
    /// The operation cannot be represented by the current Windows
    /// compatibility output path.
    #[error("Windows compatibility backend does not support {0}")]
    Unsupported(String),
}

impl WindowsError {
    /// Builds an error from the current thread's Win32 last-error value.
    #[cfg(target_os = "windows")]
    pub(crate) fn last_os_error(operation: &'static str) -> Self {
        Self::Win32 {
            operation,
            message: std::io::Error::last_os_error().to_string(),
        }
    }
}
