use dashmap::{mapref::entry::Entry, DashMap};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::Sender;
use tokio::sync::oneshot;

pub type Outbound = Sender<Vec<u8>>;

/// Delivered to a parked host once a viewer takes its registration: the
/// viewer's own sender, so the host can start forwarding its traffic there.
pub type PeerNotify = oneshot::Sender<(Outbound, String)>;

#[derive(Debug, PartialEq, Eq)]
pub enum Registration {
    Accepted,
    Rejected,
}

#[derive(Clone)]
pub struct Registry {
    hosts: Arc<DashMap<String, RegistrationState>>,
    connect_limits: Arc<ConnectLimits>,
    ownership_grace: Duration,
}

struct RegistrationState {
    token: [u8; 32],
    owner_until: Instant,
    parked: Option<(Outbound, PeerNotify)>,
    active: Option<Outbound>,
}

struct ConnectLimits {
    capacity: u32,
    refill: Duration,
    attempts: std::sync::Mutex<HashMap<(IpAddr, String), (u32, Instant)>>,
}

impl Default for ConnectLimits {
    fn default() -> Self {
        let capacity = std::env::var("RELAY_CONNECT_BURST").ok().and_then(|v| v.parse().ok()).filter(|v| *v > 0).unwrap_or(6);
        let refill = std::env::var("RELAY_CONNECT_REFILL_SECS").ok().and_then(|v| v.parse().ok()).filter(|v| *v > 0).unwrap_or(10);
        Self { capacity, refill: Duration::from_secs(refill), attempts: std::sync::Mutex::new(HashMap::new()) }
    }
}

impl Registry {
    pub fn with_ownership_grace(grace: Duration) -> Self {
        Self { hosts: Arc::new(DashMap::new()), connect_limits: Arc::new(ConnectLimits::default()), ownership_grace: grace }
    }

    pub fn register(&self, peer_id: String, token: [u8; 32], tx: Outbound, notify: PeerNotify, grace: Duration) -> Registration {
        let now = Instant::now();
        match self.hosts.entry(peer_id) {
            Entry::Occupied(mut existing) => {
                if existing.get().owner_until > now && !constant_time_eq(&existing.get().token, &token) {
                    Registration::Rejected
                } else {
                    existing.insert(RegistrationState { token, owner_until: now + grace, parked: Some((tx, notify)), active: None });
                    Registration::Accepted
                }
            }
            Entry::Vacant(slot) => {
                slot.insert(RegistrationState { token, owner_until: now + grace, parked: Some((tx, notify)), active: None });
                Registration::Accepted
            }
        }
    }
    pub fn take(&self, peer_id: &str) -> Option<(Outbound, PeerNotify)> {
        self.hosts.get_mut(peer_id).and_then(|mut entry| {
            let parked = entry.parked.take();
            if let Some((tx, _)) = parked.as_ref() {
                entry.active = Some(tx.clone());
                entry.owner_until = Instant::now() + self.ownership_grace;
            }
            parked
        })
    }

    pub fn unregister(&self, peer_id: &str, tx: &Outbound) {
        if let Entry::Occupied(mut entry) = self.hosts.entry(peer_id.to_owned()) {
            let parked_matches = entry.get().parked.as_ref().is_some_and(|(current, _)| current.same_channel(tx));
            let active_matches = entry.get().active.as_ref().is_some_and(|current| current.same_channel(tx));
            if parked_matches || active_matches {
                entry.get_mut().parked = None;
                entry.get_mut().active = None;
                entry.get_mut().owner_until = Instant::now() + self.ownership_grace;
            }
        }
    }

    pub fn allow_connect(&self, source: IpAddr, target: &str) -> bool {
        let now = Instant::now();
        let mut attempts = self.connect_limits.attempts.lock().expect("connect limits lock poisoned");
        let entry = attempts.entry((source, target.to_owned())).or_insert((self.connect_limits.capacity, now));
        let elapsed = now.duration_since(entry.1).as_secs_f64();
        let refill = elapsed * self.connect_limits.capacity as f64 / self.connect_limits.refill.as_secs_f64();
        entry.0 = ((entry.0 as f64 + refill).min(self.connect_limits.capacity as f64)) as u32;
        entry.1 = now;
        if entry.0 == 0 { false } else { entry.0 -= 1; true }
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::with_ownership_grace(Duration::from_secs(300))
    }
}

pub(crate) fn constant_time_eq(left: &[u8; 32], right: &[u8; 32]) -> bool {
    let mut difference = 0u8;
    for (a, b) in left.iter().zip(right.iter()) { difference |= a ^ b; }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::{Outbound, Registration, Registry};
    use std::time::Duration;
    use tokio::sync::{mpsc, oneshot};

    fn registration(registry: &Registry, id: &str, token: &str) -> (Registration, Outbound) {
        let (tx, _rx) = mpsc::channel(1);
        let (notify, _wait) = oneshot::channel();
        let result = registry.register(id.into(), token_from_byte(token), tx.clone(), notify, Duration::from_secs(60));
        (result, tx)
    }

    fn token_from_byte(value: &str) -> [u8; 32] { [value.as_bytes().first().copied().unwrap_or(0); 32] }

    #[test]
    fn first_registration_binds_id_to_token() {
        let registry = Registry::default();
        let (result, _) = registration(&registry, "123456789", "first");
        assert_eq!(result, Registration::Accepted);
    }

    #[test]
    fn same_token_can_reregister() {
        let registry = Registry::default();
        assert_eq!(registration(&registry, "123456789", "same").0, Registration::Accepted);
        assert_eq!(registration(&registry, "123456789", "same").0, Registration::Accepted);
    }

    #[test]
    fn different_token_cannot_replace_live_registration() {
        let registry = Registry::default();
        let (_, original_tx) = registration(&registry, "123456789", "owner");
        assert_eq!(registration(&registry, "123456789", "attacker").0, Registration::Rejected);
        let (stored_tx, _) = registry.take("123456789").unwrap();
        assert!(original_tx.same_channel(&stored_tx));
    }

    #[test]
    fn ownership_lease_blocks_reclaim_until_expiry() {
        let registry = Registry::default();
        let (_, tx) = registration(&registry, "123456789", "old");
        registry.unregister("123456789", &tx);
        assert_eq!(registration(&registry, "123456789", "new").0, Registration::Rejected);
    }

    #[test]
    fn ownership_can_be_reclaimed_after_lease_expiry() {
        let registry = Registry::default();
        let (tx, _rx) = mpsc::channel(1);
        let (notify, _wait) = oneshot::channel();
        assert_eq!(registry.register("123456789".into(), [1; 32], tx, notify, Duration::from_millis(1)), Registration::Accepted);
        std::thread::sleep(Duration::from_millis(3));
        let (new_tx, _rx) = mpsc::channel(1);
        let (new_notify, _wait) = oneshot::channel();
        assert_eq!(registry.register("123456789".into(), [2; 32], new_tx, new_notify, Duration::from_secs(1)), Registration::Accepted);
    }

    #[test]
    fn constant_time_compare_returns_correct_equality() {
        assert!(super::constant_time_eq(&[7; 32], &[7; 32]));
        assert!(!super::constant_time_eq(&[7; 32], &[8; 32]));
    }

    #[test]
    fn unregister_does_not_clobber_same_token_reregistration() {
        let registry = Registry::default();
        let (_, old_tx) = registration(&registry, "123456789", "same");
        let (_, new_tx) = registration(&registry, "123456789", "same");
        registry.unregister("123456789", &old_tx);
        let (stored_tx, _) = registry.take("123456789").unwrap();
        assert!(new_tx.same_channel(&stored_tx));
    }
}
