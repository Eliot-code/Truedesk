use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};

fn nonce_from_counter(counter: u64) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[4..].copy_from_slice(&counter.to_be_bytes());
    n
}

pub struct Sealer {
    cipher: ChaCha20Poly1305,
    counter: u64,
}

impl Sealer {
    pub fn new(key: [u8; 32]) -> Sealer {
        Sealer { cipher: ChaCha20Poly1305::new((&key).into()), counter: 0 }
    }
    pub fn seal(&mut self, aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
        let nbytes = nonce_from_counter(self.counter);
        self.counter += 1;
        let nonce = Nonce::from_slice(&nbytes);
        let ct = self.cipher.encrypt(nonce, Payload { msg: plaintext, aad }).expect("seal");
        let mut wire = Vec::with_capacity(12 + ct.len());
        wire.extend_from_slice(&nbytes);
        wire.extend_from_slice(&ct);
        wire
    }
}

/// Sliding replay window (in counters). 128 tolerates out-of-order/dropped
/// media frames over UDP while still rejecting replays. Reliable channels
/// (Data/Control) arrive in order, so they always sit at the window head.
const REPLAY_WINDOW: u64 = 128;

pub struct Opener {
    cipher: ChaCha20Poly1305,
    // Anti-replay state (RFC 6479 style). `highest` is the largest counter
    // accepted so far; `window` is a bitmap where bit i marks (highest - i) as
    // already seen. State advances ONLY for authentic frames (post-decrypt),
    // so a forged frame cannot poison the window.
    seen_any: bool,
    highest: u64,
    window: u128,
}

impl Opener {
    pub fn new(key: [u8; 32]) -> Opener {
        Opener {
            cipher: ChaCha20Poly1305::new((&key).into()),
            seen_any: false,
            highest: 0,
            window: 0,
        }
    }

    pub fn open(&mut self, aad: &[u8], wire: &[u8]) -> Option<Vec<u8>> {
        if wire.len() < 12 + 16 {
            return None;
        }
        let (nbytes, ct) = wire.split_at(12);
        let nonce = Nonce::from_slice(nbytes);
        // Authenticate first: only genuine frames may touch replay state.
        let plaintext = self.cipher.decrypt(nonce, Payload { msg: ct, aad }).ok()?;
        let counter = u64::from_be_bytes(nbytes[4..12].try_into().ok()?);
        if !self.accept_counter(counter) {
            return None; // authentic but replayed or too old — drop it.
        }
        Some(plaintext)
    }

    /// Returns true and records the counter if it is fresh; false if it is a
    /// replay or older than the window.
    fn accept_counter(&mut self, counter: u64) -> bool {
        if !self.seen_any {
            self.seen_any = true;
            self.highest = counter;
            self.window = 1;
            return true;
        }
        if counter > self.highest {
            let shift = counter - self.highest;
            self.window = if shift >= REPLAY_WINDOW {
                0
            } else {
                self.window << shift
            };
            self.window |= 1;
            self.highest = counter;
            true
        } else {
            let diff = self.highest - counter;
            if diff >= REPLAY_WINDOW {
                return false; // too old
            }
            let mask = 1u128 << diff;
            if self.window & mask != 0 {
                return false; // already seen — replay
            }
            self.window |= mask;
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_open_roundtrips() {
        let key = [7u8; 32];
        let mut s = Sealer::new(key);
        let mut o = Opener::new(key);
        let wire = s.seal(&[0x10], b"secret frame");
        assert_eq!(o.open(&[0x10], &wire).as_deref(), Some(&b"secret frame"[..]));
    }

    #[test]
    fn nonces_advance_and_stay_unique() {
        let mut s = Sealer::new([1u8; 32]);
        let a = s.seal(&[0x10], b"x");
        let b = s.seal(&[0x10], b"x");
        assert_ne!(&a[..12], &b[..12]); // nonce prefix differs
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let mut s = Sealer::new([2u8; 32]);
        let mut o = Opener::new([2u8; 32]);
        let mut wire = s.seal(&[0x10], b"data");
        let last = wire.len() - 1;
        wire[last] ^= 0xFF;
        assert!(o.open(&[0x10], &wire).is_none());
    }

    #[test]
    fn replayed_frame_is_rejected() {
        let key = [3u8; 32];
        let mut s = Sealer::new(key);
        let mut o = Opener::new(key);
        let wire = s.seal(&[0x12], b"move mouse");
        assert!(o.open(&[0x12], &wire).is_some()); // first delivery accepted
        assert!(o.open(&[0x12], &wire).is_none()); // exact replay rejected
    }

    #[test]
    fn out_of_order_within_window_is_accepted_once() {
        let key = [4u8; 32];
        let mut s = Sealer::new(key);
        let mut o = Opener::new(key);
        let f0 = s.seal(&[0x10], b"0");
        let f1 = s.seal(&[0x10], b"1");
        let f2 = s.seal(&[0x10], b"2");
        // Deliver reordered: 2, 0, 1 — all fresh, all accepted.
        assert!(o.open(&[0x10], &f2).is_some());
        assert!(o.open(&[0x10], &f0).is_some());
        assert!(o.open(&[0x10], &f1).is_some());
        // Any of them replayed is now rejected.
        assert!(o.open(&[0x10], &f0).is_none());
        assert!(o.open(&[0x10], &f2).is_none());
    }

    #[test]
    fn frame_older_than_window_is_rejected() {
        let key = [5u8; 32];
        let mut s = Sealer::new(key);
        let mut o = Opener::new(key);
        let first = s.seal(&[0x13], b"old"); // counter 0
        // Advance the sender well past the replay window.
        for _ in 0..(REPLAY_WINDOW + 5) {
            let w = s.seal(&[0x13], b"x");
            assert!(o.open(&[0x13], &w).is_some());
        }
        // The very first frame is now older than the window: dropped.
        assert!(o.open(&[0x13], &first).is_none());
    }

    #[test]
    fn different_aad_fails_but_matching_aad_succeeds() {
        let key = [6u8; 32];
        let mut s = Sealer::new(key);
        let wire = s.seal(&[0x10], b"video");
        let mut wrong_opener = Opener::new(key);
        assert!(wrong_opener.open(&[0x11], &wire).is_none());
        let mut matching_opener = Opener::new(key);
        assert_eq!(matching_opener.open(&[0x10], &wire).as_deref(), Some(&b"video"[..]));
    }
}
