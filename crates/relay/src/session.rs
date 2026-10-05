use crate::registry::{Outbound, Registry};
use protocol::{control::Control, frame, Tag};
use tokio::sync::oneshot;

pub const SUPPORTED_VERSION: u32 = 1;

#[derive(Debug, PartialEq, Eq)]
pub enum ClassifyError {
    InvalidControl,
    InvalidRegistration,
    InvalidConnect,
    ConnectRateLimited,
}

pub fn valid_peer_id(value: &str) -> bool {
    value.len() == 9 && value.bytes().all(|byte| byte.is_ascii_digit())
}

pub fn decode_token(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()) { return None; }
    let mut token = [0u8; 32];
    for (index, [high, low]) in value.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        token[index] = (hex_value(*high)? << 4) | hex_value(*low)?;
    }
    Some(token)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

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
pub fn classify(msg: &[u8], me: Outbound, reg: &Registry, source: std::net::IpAddr, ownership_grace: std::time::Duration) -> Result<Bridge, ClassifyError> {
    let (tag, body) = frame::unwrap(msg).ok_or(ClassifyError::InvalidControl)?;
    if tag != Tag::Control { return Err(ClassifyError::InvalidControl); }
    match Control::from_bytes(body).ok_or(ClassifyError::InvalidControl)? {
        Control::Register { peer_id, token, ver } => {
            let token = decode_token(&token).ok_or(ClassifyError::InvalidRegistration)?;
            if !valid_peer_id(&peer_id) || ver != SUPPORTED_VERSION { return Err(ClassifyError::InvalidRegistration); }
            let (notify_tx, notify_rx) = oneshot::channel();
            if reg.register(peer_id.clone(), token, me, notify_tx, ownership_grace) == crate::registry::Registration::Rejected {
                return Err(ClassifyError::InvalidRegistration);
            }
            Ok(Bridge::HostRegistered { peer_id, notify_rx })
        }
        Control::Connect { target, peer_id, ver } => {
            if !valid_peer_id(&target) || !valid_peer_id(&peer_id) || ver != SUPPORTED_VERSION { return Err(ClassifyError::InvalidConnect); }
            if !reg.allow_connect(source, &target) { return Err(ClassifyError::ConnectRateLimited); }
            let (host_tx, notify) = reg.take(&target).ok_or(ClassifyError::InvalidConnect)?;
            let _ = notify.send((me, peer_id.clone()));
            Ok(Bridge::ViewerPaired { host_tx, requester_id: peer_id, target_id: target })
        }
        _ => Err(ClassifyError::InvalidControl),
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
    use super::{decode_token, paired_frame, valid_peer_id};
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

    #[test]
    fn register_validation_accepts_only_bounded_wire_values() {
        assert!(valid_peer_id("123456789"));
        assert!(!valid_peer_id("123-456-789"));
        assert!(!valid_peer_id("12345678"));
        assert!(decode_token(&"ab".repeat(32)).is_some());
        assert!(decode_token(&"AB".repeat(32)).is_none());
        assert!(decode_token(&"gg".repeat(32)).is_none());
        assert!(decode_token("00").is_none());
    }
}
