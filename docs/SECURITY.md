# OpenDesk security model

OpenDesk encrypts session payloads end to end between the host and viewer.
The relay forwards traffic and is not trusted with decryption keys. This
document describes the implementation. It is not a certification, and it
offers no protection against compromised endpoints.

## Reporting a vulnerability

Do not open a public issue, discussion, or pull request for a security
problem. Report it privately through GitHub's private vulnerability reporting:
open the repository's **Security** tab and choose **Report a vulnerability**
([direct link](https://github.com/treeot/opendesk/security/advisories/new)).

Include the affected component (relay, host, viewer, or protocol), the version
or commit, steps to reproduce, and the impact you expect. We will acknowledge
the report, work on a fix in a private advisory, and credit you when the
advisory is published unless you prefer otherwise.

## Key establishment and encryption

1. The endpoints run an ephemeral SPAKE2 password-authenticated key exchange
   using the session password.
2. They exchange confirmation MACs derived from the SPAKE2 result. A wrong
   password produces a different result and fails confirmation before screen
   capture or input injection starts.
3. HKDF-SHA-256 derives separate 32-byte directional keys. The labels include
   the direction and channel (`h2v`/`v2h`, `data`, or `control`).
4. ChaCha20-Poly1305 seals each payload. Each sealer uses a per-key counter
   nonce, and because each channel has its own key, nonce sequences that start
   independently never share a key. Payloads that fail authentication are
   discarded.

The media keys cover screen video, audio, and remote input. File and
clipboard messages use the reliable Data channel and its own keys. Monitor
control messages use the Control keys. The relay fallback and the direct UDP
path carry the same sealed payloads.

TLS/WSS protects the network hop to the relay and reverse proxy, but the
end-to-end boundary is the inner ChaCha20-Poly1305 layer. That layer is what
stops the relay from decrypting session payloads.

## What the relay can see

The relay sees connection and routing metadata, including:

- client network addresses as observed by the server, connection timing,
  message sizes, and traffic volume;
- the host's register ID and the viewer's requested target ID;
- pairing/session routing messages and peer roles;
- SPAKE2 handshake messages, which do not contain the plaintext password;
- ICE credentials and candidate strings exchanged for connectivity; and
- whether the connection is active and when it closes.

The relay can also drop, delay, reorder, or replay traffic that the protocol
or transport does not reject. It can deny service, and it sees metadata even
when TLS is enabled at the edge.

## What the relay cannot decrypt

The relay never receives the SPAKE2-derived keys. As long as the endpoints and
the password are not compromised, it cannot read or forge authenticated
session payloads: screen frames, remote input, audio, file contents,
clipboard text, or sealed monitor-control payloads. It can forward those
bytes or refuse to, but the relay code does not open their AEAD ciphertext.

## Unattended access

With unattended access enabled, a successful SPAKE2 exchange using the stored
unattended password is enough for the host to accept the session without a
manual approval prompt. Anyone who obtains that password gets remote control.
If you also turn on two-factor authentication, the viewer must send a valid
TOTP code as well; a wrong code rejects the session.

The viewer stores the unattended password and the TOTP secret in the OS
keychain, so anyone who can unlock the host's user account can read them. If
the keychain read fails, the host disables unattended approval for that
session and falls back to manual approval. Use a long, unique secret and
disable unattended access when you don't need it.

Manual approval is the default when unattended access is disabled. Devices on
the host's allow-list skip the approval dialog but still have to authenticate
with the password. The pairing flow does not yet provide a cryptographic peer
identity: the relay's `Paired` message carries the ID the requester reported
about itself, which neither the relay nor the key exchange verifies, and the
approval dialog shows a placeholder fingerprint. Treat the peer ID and
password as trust-on-first-use identifiers, not as a certificate-backed
identity.

## Known limitations and threat model

- Password strength matters. SPAKE2 keeps the password off the wire and
  defeats a simple relay MITM, but an attacker who obtains the necessary
  session material or compromises an endpoint can still guess a weak password.
- A compromised host or viewer can read plaintext before sealing or after
  opening. Endpoint malware, a modified binary, malicious Tauri/WebView code,
  and attackers with local access are outside the relay-only threat model.
- The relay can deny service and learn traffic metadata. E2EE does not hide
  peer IDs, timing, sizes, ICE candidates, or network addresses from it.
- TLS certificate validation and reverse-proxy configuration still matter for
  transport security. TLS does not replace the inner E2EE layer.
- Host audio is the system output on macOS 14.6+, Windows, and Linux with a
  PulseAudio or PipeWire (pipewire-pulse) server; elsewhere the host sends
  no audio. The host never captures the microphone.
- Windows and Linux host support is less mature and has not been
  runtime-tested across all capture, permission, and input-injection
  environments.
- OpenDesk is under active development. Review the source, test the exact
  binaries you deploy, and don't treat this document as a security audit.
