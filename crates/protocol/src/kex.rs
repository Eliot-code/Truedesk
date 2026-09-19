use hkdf::Hkdf;
use sha2::Sha256;

fn hkdf32(secret: &[u8], label: &[u8]) -> [u8; 32] {
    let hk = Hkdf::<Sha256>::new(None, secret);
    let mut out = [0u8; 32];
    hk.expand(label, &mut out).expect("32 bytes fits");
    out
}

/// Returns (send_key, recv_key) for this peer.
pub fn derive_keys(shared_secret: &[u8], role_is_host: bool) -> ([u8; 32], [u8; 32]) {
    let h2v = hkdf32(shared_secret, b"opendesk h2v");
    let v2h = hkdf32(shared_secret, b"opendesk v2h");
    if role_is_host {
        (h2v, v2h) // host sends h2v, receives v2h
    } else {
        (v2h, h2v) // viewer sends v2h, receives h2v
    }
}

/// Returns (send_key, recv_key) for the reliable DATA channel (file transfer,
/// clipboard, ...). Distinct labels give the Data channel a completely separate
/// keyspace from the media channel, so their independent nonce counters can
/// never collide under a shared key (which would be catastrophic AEAD nonce
/// reuse). Directional like `derive_keys`.
pub fn derive_data_keys(shared_secret: &[u8], role_is_host: bool) -> ([u8; 32], [u8; 32]) {
    let h2v = hkdf32(shared_secret, b"opendesk data h2v");
    let v2h = hkdf32(shared_secret, b"opendesk data v2h");
    if role_is_host {
        (h2v, v2h)
    } else {
        (v2h, h2v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_keys_agree_and_differ_from_media() {
        let shared = [7u8; 32];
        let (host_send, host_recv) = derive_data_keys(&shared, true);
        let (viewer_send, viewer_recv) = derive_data_keys(&shared, false);
        // Data channel directional agreement (same as media).
        assert_eq!(host_send, viewer_recv);
        assert_eq!(viewer_send, host_recv);
        // Data keys MUST differ from the media keys (separate keyspace -> no
        // nonce reuse across the two streams under a shared counter start).
        let (media_send, media_recv) = derive_keys(&shared, true);
        assert_ne!(host_send, media_send);
        assert_ne!(host_recv, media_recv);
    }

    #[test]
    fn host_and_viewer_agree_on_directional_keys() {
        let shared = [42u8; 32];
        let (host_send, host_recv) = derive_keys(&shared, true);
        let (viewer_send, viewer_recv) = derive_keys(&shared, false);
        // host->viewer channel: host sends with the key viewer receives with
        assert_eq!(host_send, viewer_recv);
        // viewer->host channel
        assert_eq!(viewer_send, host_recv);
    }

    #[test]
    fn directions_use_distinct_keys() {
        let (send, recv) = derive_keys(&[9u8; 32], true);
        assert_ne!(send, recv);
    }
}
