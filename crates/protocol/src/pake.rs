use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use spake2::{Ed25519Group, Identity, Password, Spake2};

type HmacSha256 = Hmac<Sha256>;

const ID_HOST: &[u8] = b"opendesk-host";
const ID_VIEWER: &[u8] = b"opendesk-viewer";
// Fixed transcript label the MAC covers; both ends use the same base.
const CONFIRM_TRANSCRIPT: &[u8] = b"opendesk key confirmation v1";

/// Opaque SPAKE2 handshake state. Held between `start` and `finish`.
pub struct PakeState(Spake2<Ed25519Group>);

/// Begin the handshake. Returns opaque state and the message to send as 0x02|0x01.
pub fn start(password: &[u8], role_is_host: bool) -> (PakeState, Vec<u8>) {
    let pw = Password::new(password);
    let a = Identity::new(ID_HOST);
    let b = Identity::new(ID_VIEWER);
    let (state, msg) = if role_is_host {
        Spake2::<Ed25519Group>::start_a(&pw, &a, &b)
    } else {
        Spake2::<Ed25519Group>::start_b(&pw, &a, &b)
    };
    (PakeState(state), msg)
}

/// Complete the handshake against the peer's 0x02|0x01 message.
/// Returns the raw SPAKE2 shared output (feed into kex::derive_keys).
pub fn finish(state: PakeState, peer_msg: &[u8]) -> Option<[u8; 32]> {
    let key = state.0.finish(peer_msg).ok()?;
    Some(normalize(&key))
}

/// Ed25519Group SPAKE2 output is 32 bytes; hash-normalize defensively.
fn normalize(raw: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    Hkdf::<Sha256>::new(None, raw)
        .expand(b"opendesk spake2 normalize", &mut out)
        .expect("32 fits");
    out
}

fn confirm_key(shared: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    Hkdf::<Sha256>::new(None, shared)
        .expand(b"opendesk confirm", &mut out)
        .expect("32 fits");
    out
}

fn tag_for(shared: &[u8], role_is_host: bool) -> [u8; 32] {
    let ck = confirm_key(shared);
    let mut mac = HmacSha256::new_from_slice(&ck).expect("hmac key");
    mac.update(CONFIRM_TRANSCRIPT);
    mac.update(if role_is_host { b"host" } else { b"viewer" });
    let bytes = mac.finalize().into_bytes();
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    out
}

/// The confirm MAC this side sends (body for 0x02|0x02).
pub fn confirm_tag(shared: &[u8], role_is_host: bool) -> [u8; 32] {
    tag_for(shared, role_is_host)
}

/// Verify the peer's confirm MAC. `peer_is_host` = the sender's role.
/// Constant-time comparison via HMAC verify semantics.
pub fn verify_peer(shared: &[u8], peer_is_host: bool, peer_tag: &[u8]) -> bool {
    if peer_tag.len() != 32 {
        return false;
    }
    let ck = confirm_key(shared);
    let mut mac = HmacSha256::new_from_slice(&ck).expect("hmac key");
    mac.update(CONFIRM_TRANSCRIPT);
    mac.update(if peer_is_host { b"host" } else { b"viewer" });
    // Constant-time comparison against the peer's tag.
    mac.verify_slice(peer_tag).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_and_viewer_agree_with_same_password() {
        let (hs, h_msg) = start(b"hunter2", true);
        let (vs, v_msg) = start(b"hunter2", false);
        let h_key = finish(hs, &v_msg).expect("host finishes");
        let v_key = finish(vs, &h_msg).expect("viewer finishes");
        assert_eq!(h_key, v_key, "same password => same shared key");
    }

    #[test]
    fn wrong_password_produces_different_key_and_confirm_fails() {
        let (hs, h_msg) = start(b"hunter2", true);
        let (vs, v_msg) = start(b"WRONG", false);
        let h_key = finish(hs, &v_msg).expect("host finishes");
        let v_key = finish(vs, &h_msg).expect("viewer finishes");
        assert_ne!(h_key, v_key);
        // host verifying viewer's confirm tag must fail
        let v_confirm = confirm_tag(&v_key, false);
        assert!(!verify_peer(&h_key, false, &v_confirm));
    }

    #[test]
    fn confirm_tags_roundtrip_on_match() {
        let (hs, h_msg) = start(b"pw", true);
        let (vs, v_msg) = start(b"pw", false);
        let h_key = finish(hs, &v_msg).unwrap();
        let v_key = finish(vs, &h_msg).unwrap();
        let h_confirm = confirm_tag(&h_key, true);
        let v_confirm = confirm_tag(&v_key, false);
        assert!(verify_peer(&v_key, true, &h_confirm)); // viewer verifies host
        assert!(verify_peer(&h_key, false, &v_confirm)); // host verifies viewer
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Known-answer test: pins the HKDF/HMAC-SHA256 confirm-tag derivation and
    /// SPAKE2 output normalization. Changing these breaks wire compatibility.
    /// Expected values cross-checked against an independent implementation.
    #[test]
    fn confirm_and_normalize_known_answers() {
        let shared: Vec<u8> = (0u8..32).collect();
        assert_eq!(
            hex(&normalize(&shared)),
            "bfa8894ed08648fc3ab1ec223cfdf806d8644eef5760358949a38b4e496e2553"
        );
        assert_eq!(
            hex(&confirm_tag(&shared, true)),
            "86cceeb28a8fbb4ff40e982bce66eaa66d2b0b65a5109838e1916748e0a7de5c"
        );
        assert_eq!(
            hex(&confirm_tag(&shared, false)),
            "6d996a18ae26675c39a167bbb43c35db36220cfbde48449915b7fe021cc94349"
        );
        assert!(verify_peer(&shared, true, &confirm_tag(&shared, true)));
    }

    #[test]
    fn different_sessions_produce_different_keys() {
        let (hs1, _) = start(b"pw", true);
        let (_vs1, v1) = start(b"pw", false);
        let k1 = finish(hs1, &v1).unwrap();
        let (hs2, _) = start(b"pw", true);
        let (_vs2, v2) = start(b"pw", false);
        let k2 = finish(hs2, &v2).unwrap();
        assert_ne!(k1, k2, "fresh ephemeral scalars => fresh key per session");
    }
}

#[cfg(test)]
mod _sendsync_assert {
    fn _assert<T: Send + Sync + 'static>() {}
    #[allow(dead_code)]
    fn _check() { _assert::<super::PakeState>(); }
}
