---
title: Security model
description: What OpenDesk encrypts, what the relay can and can't see, and the limits you should know about.
---

OpenDesk encrypts sessions end to end between the host and the viewer. The
relay forwards the traffic but never gets the keys. This page summarises how
that works and where it stops. It describes the code, not an audit; the full
version is
[`docs/SECURITY.md`](https://github.com/treeot/opendesk/blob/main/docs/SECURITY.md).

## How a session is keyed

1. **SPAKE2.** The host and viewer run a password-authenticated key exchange
   using the session password (or the unattended password). The password
   itself never crosses the network, and a relay in the middle can't swap in
   its own key.
2. **Key confirmation.** Both sides prove they reached the same result. With
   a wrong password this fails before any screen capture or input starts.
3. **Separate keys.** HKDF-SHA-256 derives separate keys for each direction
   and each channel: media, data, and control.
4. **ChaCha20-Poly1305** seals every message. Anything that fails
   authentication is dropped.

Screen video, sound and input use the media keys. Files and clipboard text
use the data keys. Direct UDP and the relay carry the same sealed messages.

TLS (`wss://`) protects the connection to the relay as well, but the
end-to-end protection is the inner layer. That's what stops whoever runs the
relay from reading sessions.

## What the relay can see

- IP addresses, connection times, message sizes and traffic volume.
- The host's ID and the ID a viewer asks for.
- The SPAKE2 messages (which don't contain the password).
- The network addresses (ICE candidates) the two sides exchange to find a
  direct path.

It can also drop, delay or refuse traffic, so it can stop a session from
working.

## What the relay can't see

As long as neither computer and the password are compromised, the relay
can't read or forge: the screen, input, sound, files, clipboard text, or
monitor-switching messages. It only forwards them.

## Unattended access

With [unattended access](/docs/unattended/) on, the right password is enough
to get in without anyone clicking Allow, so anyone who learns it gets
control. A 2FA code adds a second factor. The password and 2FA secret live in
the OS keychain; anyone who can unlock your user account can read them.

## Known limits

- **Peer IDs aren't verified.** The ID shown in the approval dialog is what
  the connecting device says about itself. Neither the relay nor the key
  exchange proves it. That's why **Known device** only marks an ID and never
  skips approval. Trust the password, not the ID.
- **Weak passwords can be guessed.** SPAKE2 keeps the password off the wire,
  but every connection attempt is still a guess, so a short unattended
  password is a risk. The host
  slows down and rotates its session password after repeated failures.
- **Compromised computers.** Malware on the host or viewer, or a modified
  build, sees everything before it's encrypted. That's outside what OpenDesk
  can protect.
- **The browser viewer is fetched on every visit.** See
  [what you're trusting](/docs/browser-viewer/#what-youre-trusting).
- **Unsigned builds.** The release builds carry no verified publisher
  identity. Download them only from the project's releases page. App updates
  are signed with the project's update key and checked before they install.
- **Beta software.** Review the source and test the builds you deploy.

## Report a vulnerability

Please don't open a public issue. Use GitHub's
[private vulnerability reporting](https://github.com/treeot/opendesk/security/advisories/new)
and include the component, the version, steps to reproduce, and the impact.
