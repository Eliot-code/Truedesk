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
keychain, so anyone who can unlock the host's user account can read them.
Nothing is written to disk in plaintext when the keychain is unavailable. If
it can't be read (locked, wiped, moved to a new machine, or no Secret Service
on Linux), the app says so, each connection falls back to manual approval,
and a launch at sign-in opens the window instead of hiding in the tray. Use a
long, unique secret and disable unattended access when you don't need it.

"Start OpenDesk when I sign in" adds a per-user sign-in entry (a LaunchAgent
on macOS, the `Run` registry key on Windows, an XDG autostart file on Linux).
It starts the app hidden in the tray and hosting, but only after someone signs
in: nothing runs at the login screen or as a system service. It can only be
turned on once unattended access is saved. While it is on, closing the window
leaves the host running; use Quit in the tray menu to stop it.

Manual approval is the default when unattended access is disabled. Devices on
the host's allow-list skip the approval dialog but still have to authenticate
with the password. The pairing flow does not yet provide a cryptographic peer
identity: the relay's `Paired` message carries the ID the requester reported
about itself, which neither the relay nor the key exchange verifies, and the
approval dialog shows a placeholder fingerprint. Treat the peer ID and
password as trust-on-first-use identifiers, not as a certificate-backed
identity.

## Virtual displays on Windows

Windows has no built-in way to add a display, so OpenDesk uses Amyuni's
free `usbmmidd_v2` indirect display driver, the one RustDesk uses. It is
third-party code running in Windows' user-mode driver host, signed by
Microsoft through the Windows Hardware Compatibility Program. OpenDesk
doesn't ship it. **Install virtual display driver** on Home (shown only
when "Let viewers add virtual displays" is on) downloads it from
`amyuni.com` over HTTPS and refuses any file whose SHA-256 differs from the
one pinned in `viewer/src-tauri/src/vdriver.rs`. It then runs itself as
administrator (Windows asks first), checks the hash again, unpacks only the
INF, catalog and 64-bit DLL into a folder that only administrators can
write to, and installs them through Windows' own driver installer, which
also checks the Microsoft signature. Amyuni's bundled installer is never
run. **Uninstall driver** removes the device.

The driver lets anyone signed in to the computer, not just administrators,
add and remove its displays. OpenDesk only removes the ones it added, newest
first, and leaves a display in place rather than take away one that another
program added after it. The package's licence mentions an "advertisement
page". Neither the driver nor anything OpenDesk installs from the package
can show one: the driver has no networking or UI code, and Amyuni's
installer, which OpenDesk skips, only shows a standard error box.

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
