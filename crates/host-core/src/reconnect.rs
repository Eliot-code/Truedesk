use std::time::Duration;

pub const MAX_RECONNECT_ATTEMPTS: u32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisconnectReason {
    Transient,
    CleanUserDisconnect,
    AuthFailure,
}

pub fn backoff_delay(attempt: u32) -> Duration {
    Duration::from_millis(500u64.saturating_mul(2u64.saturating_pow(attempt.min(3))))
}

pub fn should_reconnect(reason: DisconnectReason) -> bool {
    matches!(reason, DisconnectReason::Transient)
}

pub fn classify_viewer_error(error: &str) -> DisconnectReason {
    let lower = error.to_ascii_lowercase();
    if lower.contains("invalid spake2") || lower.contains("key confirmation failed")
        || lower.contains("session rejected") || lower.contains("auth") {
        DisconnectReason::AuthFailure
    } else if lower.contains("connection closed") || lower.contains("transport")
        || lower.contains("channel") || lower.contains("relay") {
        DisconnectReason::Transient
    } else if lower.contains("user") || lower.contains("clean") {
        DisconnectReason::CleanUserDisconnect
    } else {
        // Errors reaching this classifier come from a failed session attempt;
        // unknown attempt failures are safer to treat as transient than to
        // strand a viewer after a relay/transport implementation changes its
        // wording.
        DisconnectReason::Transient
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_is_capped_at_five_attempts() {
        assert_eq!(backoff_delay(0), Duration::from_millis(500));
        assert_eq!(backoff_delay(1), Duration::from_secs(1));
        assert_eq!(backoff_delay(2), Duration::from_secs(2));
        assert_eq!(backoff_delay(3), Duration::from_secs(4));
        assert_eq!(backoff_delay(4), Duration::from_secs(4));
    }

    #[test]
    fn classifier_only_retries_transient_disconnects() {
        assert!(should_reconnect(DisconnectReason::Transient));
        assert!(!should_reconnect(DisconnectReason::CleanUserDisconnect));
        assert!(!should_reconnect(DisconnectReason::AuthFailure));
    }

    #[test]
    fn viewer_error_mapping_preserves_retry_boundary() {
        assert_eq!(classify_viewer_error("connection closed during KEX"), DisconnectReason::Transient);
        assert_eq!(classify_viewer_error("relay connection timed out"), DisconnectReason::Transient);
        assert_eq!(classify_viewer_error("invalid SPAKE2 message"), DisconnectReason::AuthFailure);
        assert_eq!(classify_viewer_error("key confirmation failed"), DisconnectReason::AuthFailure);
        assert_eq!(classify_viewer_error("session rejected"), DisconnectReason::AuthFailure);
        assert_eq!(classify_viewer_error("user ended session"), DisconnectReason::CleanUserDisconnect);
    }
}
