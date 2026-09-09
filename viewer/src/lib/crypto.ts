import sodium from "libsodium-wrappers";

/**
 * PoC key: k_h2v = HKDF-SHA256(secret=[42u8; 32], salt=None, info="opendesk h2v"),
 * 32-byte output. This matches the host-agent (crates/host-agent/src/main.rs):
 *
 *   let (send_key, _recv_key) = derive_keys(&[42u8; 32], true);
 *
 * where derive_keys (crates/protocol/src/kex.rs) does:
 *
 *   fn hkdf32(secret: &[u8], label: &[u8]) -> [u8; 32] {
 *       let hk = Hkdf::<Sha256>::new(None, secret);
 *       ... hk.expand(label, &mut okm) ...
 *   }
 *   let h2v = hkdf32(shared_secret, b"opendesk h2v");
 *
 * Since the host is `role_is_host = true`, its `send_key` is `h2v`. The viewer
 * is the h2v *receiver*, so it must open with this same value. Computed once
 * with Node's crypto.hkdfSync("sha256", Buffer.alloc(32, 42), Buffer.alloc(0),
 * Buffer.from("opendesk h2v"), 32) and pinned here as a constant for the PoC.
 * Replace with a live X25519 + SPAKE2 derived shared secret in a later plan.
 */
export const K_H2V = new Uint8Array([
  59, 235, 10, 159, 47, 34, 102, 57, 43, 56, 235, 199, 166, 46, 189, 101, 155,
  9, 154, 17, 216, 28, 123, 74, 201, 145, 160, 145, 75, 176, 102, 72,
]);

export type Opener = (wire: Uint8Array) => Uint8Array;

/**
 * Builds an opener for sealed video bodies: wire = nonce(12) || ciphertext || tag(16).
 * Uses libsodium's IETF ChaCha20-Poly1305 variant (12-byte nonce), matching the
 * Rust `chacha20poly1305::ChaCha20Poly1305` crate used by the host-agent/protocol.
 */
export async function makeOpener(key: Uint8Array): Promise<Opener> {
  await sodium.ready;
  return (wire: Uint8Array): Uint8Array => {
    const nonce = wire.subarray(0, 12);
    const ct = wire.subarray(12);
    return sodium.crypto_aead_chacha20poly1305_ietf_decrypt(null, ct, null, nonce, key);
  };
}
