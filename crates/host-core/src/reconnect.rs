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
}
