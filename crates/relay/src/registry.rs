use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;

pub type Outbound = UnboundedSender<Vec<u8>>;

/// Delivered to a parked host once a viewer takes its registration: the
/// viewer's own sender, so the host can start forwarding its traffic there.
pub type PeerNotify = oneshot::Sender<Outbound>;

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
}
