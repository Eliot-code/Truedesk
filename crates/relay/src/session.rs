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
    HostRegistered(oneshot::Receiver<Outbound>),
    /// This connection is a viewer bridged to the given host sender.
    ViewerPaired(Outbound),
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
        Control::Register { peer_id, .. } => {
            let (notify_tx, notify_rx) = oneshot::channel();
            reg.register(peer_id, me, notify_tx);
            Some(Bridge::HostRegistered(notify_rx))
        }
        Control::Connect { target, .. } => {
            let (host_tx, notify) = reg.take(&target)?;
            let _ = notify.send(me);
            Some(Bridge::ViewerPaired(host_tx))
        }
        _ => None,
    }
}

/// Builds the `paired` Control frame sent to each side once bridged.
pub fn paired_frame(peer_role: &str) -> Vec<u8> {
    frame::wrap(
        Tag::Control,
        &Control::Paired {
            session: "s1".into(),
            peer_role: peer_role.into(),
        }
        .to_bytes(),
    )
}
