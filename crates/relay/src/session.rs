use crate::registry::{Outbound, Registry};
use protocol::{control::Control, frame, Tag};
use tokio::sync::oneshot;

/// Result of classifying the first message received on a fresh connection.
/// Pairing rule: a `register` parks the sender in the registry (this
/// connection is a host waiting to be taken); a matching `connect` takes it
/// and bridges both directions immediately.
pub enum Bridge {
    /// This connection is a host that just parked its sender. Resolves to
    /// the viewer's sender once a matching `connect` takes it.
    HostRegistered { peer_id: String, notify_rx: oneshot::Receiver<(Outbound, String)> },
    /// This connection is a viewer bridged to the given host sender.
    ViewerPaired { host_tx: Outbound, requester_id: String, target_id: String },
}

/// Reads the classification of the first frame. Returns `None` when the
/// frame is not a valid Control frame, or a `connect` targets an unknown
/// peer_id — either way the connection should be dropped.
pub fn classify(msg: &[u8], me: Outbound, reg: &Registry) -> Option<Bridge> {
    let (tag, body) = frame::unwrap(msg)?;
    if tag != Tag::Control {
        return None;
    }
    match Control::from_bytes(body)? {
        Control::Register { peer_id, token, .. } => {
            let (notify_tx, notify_rx) = oneshot::channel();
            if reg.register(peer_id.clone(), token, me, notify_tx) == crate::registry::Registration::Rejected {
                return None;
            }
            Some(Bridge::HostRegistered { peer_id, notify_rx })
        }
        Control::Connect { target, peer_id, .. } => {
            let (host_tx, notify) = reg.take(&target)?;
            let _ = notify.send((me, peer_id.clone()));
            Some(Bridge::ViewerPaired { host_tx, requester_id: peer_id, target_id: target })
        }
        _ => None,
    }
}

/// Builds the `paired` Control frame sent to each side once bridged.
pub fn paired_frame(peer_role: &str, peer_id: &str) -> Vec<u8> {
    frame::wrap(
        Tag::Control,
        &Control::Paired {
            session: "s1".into(),
            peer_role: peer_role.into(),
            peer_id: peer_id.into(),
        }
        .to_bytes(),
    )
}

#[cfg(test)]
mod tests {
    use super::paired_frame;
    use protocol::{control::Control, frame, Tag};

    #[test]
    fn pairing_frame_carries_requester_id() {
        let bytes = paired_frame("host", "viewer-1");
        let (tag, body) = frame::unwrap(&bytes).unwrap();
        assert_eq!(tag, Tag::Control);
        assert_eq!(Control::from_bytes(body), Some(Control::Paired {
            session: "s1".into(), peer_role: "host".into(), peer_id: "viewer-1".into(),
        }));
    }
}
