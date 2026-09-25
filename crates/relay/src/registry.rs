use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::mpsc::Sender;
use tokio::sync::oneshot;

pub type Outbound = Sender<Vec<u8>>;

/// Delivered to a parked host once a viewer takes its registration: the
/// viewer's own sender, so the host can start forwarding its traffic there.
pub type PeerNotify = oneshot::Sender<(Outbound, String)>;

#[derive(Clone, Default)]
pub struct Registry {
    hosts: Arc<DashMap<String, (Outbound, PeerNotify)>>,
}

impl Registry {
    pub fn register(&self, peer_id: String, tx: Outbound, notify: PeerNotify) {
        self.hosts.insert(peer_id, (tx, notify));
    }
    pub fn take(&self, peer_id: &str) -> Option<(Outbound, PeerNotify)> {
        self.hosts.remove(peer_id).map(|(_, v)| v)
    }

    pub fn unregister(&self, peer_id: &str, tx: &Outbound) {
        if let Some(entry) = self.hosts.get(peer_id) {
            if entry.value().0.same_channel(tx) {
                drop(entry);
                self.hosts.remove(peer_id);
            }
        }
    }
}
