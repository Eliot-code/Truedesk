use chacha20poly1305::aead::{Aead, KeyInit};
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
    pub fn seal(&mut self, plaintext: &[u8]) -> Vec<u8> {
        let nbytes = nonce_from_counter(self.counter);
        self.counter += 1;
        let nonce = Nonce::from_slice(&nbytes);
        let ct = self.cipher.encrypt(nonce, plaintext).expect("seal");
        let mut wire = Vec::with_capacity(12 + ct.len());
        wire.extend_from_slice(&nbytes);
        wire.extend_from_slice(&ct);
        wire
    }
}

pub struct Opener {
    cipher: ChaCha20Poly1305,
}

impl Opener {
    pub fn new(key: [u8; 32]) -> Opener {
        Opener { cipher: ChaCha20Poly1305::new((&key).into()) }
    }
    pub fn open(&mut self, wire: &[u8]) -> Option<Vec<u8>> {
        if wire.len() < 12 + 16 {
            return None;
        }
        let (nbytes, ct) = wire.split_at(12);
        let nonce = Nonce::from_slice(nbytes);
        self.cipher.decrypt(nonce, ct).ok()
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
        let wire = s.seal(b"secret frame");
        assert_eq!(o.open(&wire).as_deref(), Some(&b"secret frame"[..]));
    }

    #[test]
    fn nonces_advance_and_stay_unique() {
        let mut s = Sealer::new([1u8; 32]);
        let a = s.seal(b"x");
        let b = s.seal(b"x");
        assert_ne!(&a[..12], &b[..12]); // nonce prefix differs
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let mut s = Sealer::new([2u8; 32]);
        let mut o = Opener::new([2u8; 32]);
        let mut wire = s.seal(b"data");
        let last = wire.len() - 1;
        wire[last] ^= 0xFF;
        assert!(o.open(&wire).is_none());
    }
}
