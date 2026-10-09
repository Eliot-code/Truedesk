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

pub fn derive_control_keys(shared_secret: &[u8], role_is_host: bool) -> ([u8; 32], [u8; 32]) {
    let h2v = hkdf32(shared_secret, b"opendesk control h2v");
    let v2h = hkdf32(shared_secret, b"opendesk control v2h");
    if role_is_host { (h2v, v2h) } else { (v2h, h2v) }
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

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Known-answer test: pins HKDF-SHA256 key derivation for every channel.
    /// Changing these outputs breaks wire compatibility with existing peers.
    /// Expected values cross-checked against an independent implementation.
    #[test]
    fn derived_keys_match_known_answers() {
        let shared: Vec<u8> = (0u8..32).collect();
        let (media_h2v, media_v2h) = derive_keys(&shared, true);
        let (data_h2v, data_v2h) = derive_data_keys(&shared, true);
        let (ctl_h2v, ctl_v2h) = derive_control_keys(&shared, true);
        assert_eq!(
            hex(&media_h2v),
            "f40456bde2848f896b16d1bd9135e8b3e1ab6b127e3965cc87e541bd807e0345"
        );
        assert_eq!(
            hex(&media_v2h),
            "fe007c704def68ca52b10704bca4686a2b6ab3a71e23c4fb6dd35a43d590ccc6"
        );
        assert_eq!(
            hex(&data_h2v),
            "8d233877fd7cfdd719a52e658c06889c20b5bea6da667d0d8f8fdc89ccfdee25"
        );
        assert_eq!(
            hex(&data_v2h),
            "e4f260c2845248e36db213631ecedc873e69cb5dff37f4b8a01978d9758198d6"
        );
        assert_eq!(
            hex(&ctl_h2v),
            "e07e123f85c2d4eeef3a0dcca10e2df36f5a643ee3642ba88b6d447f97ecaff7"
        );
        assert_eq!(
            hex(&ctl_v2h),
            "a7af61489ce510f282d14798649e3c4678f5b84a5b2b5310f6b51b4d31db5774"
        );
    }

    #[test]
    fn directions_use_distinct_keys() {
        let (send, recv) = derive_keys(&[9u8; 32], true);
        assert_ne!(send, recv);
    }
}
