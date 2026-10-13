//! Delivering the secure attention sequence (Ctrl+Alt+Del) on the host.
//!
//! A viewer sends `Control::RequestSecureAttention` and the host calls
//! [`deliver`]. On Windows the security screen is a protected desktop that an
//! ordinary process cannot reach; reaching it is only possible from the
//! helper service that runs at the sign-in screen. That service is not built
//! yet (see `docs/WINDOWS_SERVICE.md`), so [`deliver`] is a scaffold: it
//! reports [`Unsupported`] today and gains a real implementation once the
//! service exists.

/// Why the secure attention sequence could not be delivered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecureAttentionError {
    /// This platform has no secure attention sequence, or OpenDesk cannot
    /// reach it yet from where it runs.
    Unsupported,
}

impl std::fmt::Display for SecureAttentionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported => write!(f, "secure attention is not available on this host"),
        }
    }
}

impl std::error::Error for SecureAttentionError {}

/// Deliver Ctrl+Alt+Del on this host. Succeeds only where OpenDesk can reach
/// the secure attention sequence; everywhere else it returns [`Unsupported`]
/// and the caller simply ignores the request.
pub fn deliver() -> Result<(), SecureAttentionError> {
    // Scaffold: the Windows path needs the sign-in-screen helper service,
    // which is not built yet. Until then every platform reports Unsupported.
    Err(SecureAttentionError::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deliver_is_unsupported_until_the_service_exists() {
        assert_eq!(deliver(), Err(SecureAttentionError::Unsupported));
    }
}
