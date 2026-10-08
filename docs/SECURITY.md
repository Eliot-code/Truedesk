# OpenDesk security model

OpenDesk protects session payloads end to end between the host and viewer.
The relay is a forwarding service, not a trusted decryption service. This is
an implementation description, not a certification or a guarantee against
compromised endpoints.

## Reporting a vulnerability

Please do not open a public issue, discussion, or pull request for a security
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
4. Payloads are sealed with ChaCha20-Poly1305. Each sealer uses a per-key
   counter nonce, and distinct channel keys keep the independently-started
   nonce sequences in separate key spaces. Authentication failures discard the
   payload.

The media key space carries screen video, audio, and remote input.
File and clipboard messages use the reliable Data channel and its separate
keys. Monitor control messages use the separate Control keys. The relay
fallback and direct UDP path carry the same sealed payloads.

TLS/WSS protects the network hop to the relay and reverse proxy, but TLS is
not the real end-to-end boundary: the inner ChaCha20-Poly1305 layer is what
prevents the relay from decrypting session payloads.

## What the relay can see

The relay necessarily sees connection and routing metadata, including:

- client network addresses as observed by the server, connection timing,
  message sizes, and traffic volume;
- the host's register ID and the viewer's requested target ID;
- pairing/session routing messages and peer roles;
- SPAKE2 handshake messages, which do not contain the plaintext password;
- ICE credentials and candidate strings exchanged for connectivity; and
- whether the connection is active and when it closes.

The relay can also drop, delay, reorder, or replay traffic that the protocol
or transport does not reject. It can deny service and can observe metadata
even when TLS is enabled at the edge.

## What the relay cannot decrypt

The relay does not receive the SPAKE2-derived keys. Assuming the endpoints and
their password are not compromised, it cannot read or create authenticated
session payloads, including screen frames, remote input, audio,
file contents, clipboard text, or sealed monitor-control payloads. It can
forward those bytes, or refuse to forward them, but the relay code does not
open their AEAD ciphertext.

## Unattended access

When unattended access is enabled, a successful SPAKE2 exchange with the
stored unattended password is sufficient for the host session to be accepted
without a manual approval prompt. This is equivalent to granting remote
control to anyone who obtains that password. The current viewer stores the
unattended password in its local JSON configuration; it is not protected by
an OS keychain in the inspected implementation. Use a long, unique secret,
restrict filesystem access, and disable unattended access when it is not
needed.

Manual approval remains the default when unattended access is disabled. The
current pairing flow does not yet provide a complete cryptographic peer
identity or enforced allow-list decision: the relay's current `Paired`
message does not carry the requester's ID, and the UI may show placeholder
peer details. Treat the peer ID and password as trust-on-first-use application
identifiers, not as a certificate-backed identity.

## Known limitations and threat model

- Password strength is security-critical. SPAKE2 protects the exchange from
  sending the password and from a simple relay MITM, but a weak password is
  still vulnerable to guessing if an attacker obtains the necessary session
  material or compromises an endpoint.
- A compromised host or viewer can read plaintext before sealing or after
  opening. Endpoint malware, a modified binary, malicious Tauri/WebView code,
  or an attacker with local access is outside the relay-only threat model.
- The relay can deny service and learn traffic metadata. E2EE does not hide
  peer IDs, timing, sizes, ICE candidates, or network addresses from it.
- TLS certificate validation and reverse-proxy configuration still matter for
  transport security. TLS does not replace the inner E2EE layer.
- There is no implemented two-factor authentication.
- The unattended password (and TOTP secret) live in the OS keychain, so
  anyone who can unlock the host's user account can read them.
- Host audio is the system output on macOS 14.6+ and Windows; elsewhere it
  falls back to the default microphone.
- Windows and Linux host support is not equally mature or fully runtime-tested
  across all capture, permission, and input-injection environments.
- The current implementation is under active development. Review the source,
  test the exact binaries you deploy, and do not treat this document as a
  security audit.

