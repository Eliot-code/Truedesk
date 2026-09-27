use dashmap::{mapref::entry::Entry, DashMap};
use std::sync::Arc;
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

#[derive(Clone, Default)]
pub struct Registry {
    hosts: Arc<DashMap<String, (String, Outbound, PeerNotify)>>,
}

impl Registry {
    pub fn register(&self, peer_id: String, token: String, tx: Outbound, notify: PeerNotify) -> Registration {
        match self.hosts.entry(peer_id) {
            Entry::Occupied(mut existing) => {
                if existing.get().0 != token {
                    Registration::Rejected
                } else {
                    existing.insert((token, tx, notify));
                    Registration::Accepted
                }
            }
            Entry::Vacant(slot) => {
                slot.insert((token, tx, notify));
                Registration::Accepted
            }
        }
    }
    pub fn take(&self, peer_id: &str) -> Option<(Outbound, PeerNotify)> {
        self.hosts.remove(peer_id).map(|(_, (_, tx, notify))| (tx, notify))
    }

    pub fn unregister(&self, peer_id: &str, tx: &Outbound) {
        if let Some(entry) = self.hosts.get(peer_id) {
            if entry.value().1.same_channel(tx) {
                drop(entry);
                self.hosts.remove(peer_id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Outbound, Registration, Registry};
    use tokio::sync::{mpsc, oneshot};

    fn registration(registry: &Registry, id: &str, token: &str) -> (Registration, Outbound) {
        let (tx, _rx) = mpsc::channel(1);
        let (notify, _wait) = oneshot::channel();
        let result = registry.register(id.into(), token.into(), tx.clone(), notify);
        (result, tx)
    }

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
    fn released_id_can_be_claimed_by_new_token() {
        let registry = Registry::default();
        let (_, tx) = registration(&registry, "123456789", "old");
        registry.unregister("123456789", &tx);
        assert_eq!(registration(&registry, "123456789", "new").0, Registration::Accepted);
    }
}
