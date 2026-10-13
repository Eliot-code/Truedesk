# Windows sign-in screen, UAC and Ctrl+Alt+Del

Reaching the Windows sign-in screen, UAC prompts and the lock screen needs a
small helper that runs with system privileges and launches the host inside the
active console session. RustDesk, TeamViewer and AnyDesk all work this way.
OpenDesk ships the plumbing for it; the privileged pieces are not implemented
and are the work this document describes.

## What is in place (scaffold)

- `Control::RequestSecureAttention` — the viewer asks the host to deliver
  Ctrl+Alt+Del (`crates/protocol/src/control.rs`). Old peers ignore it.
- `host_core::secure_attention::deliver()` — called when the host receives
  that message (`crates/host-core/src/secure_attention.rs`). It returns
  `Unsupported` today.
- `send_secure_attention` — the Tauri command and the `sendSecureAttention`
  API binding the viewer UI calls.

## What is not implemented

These are the privileged parts. They are left for a contributor to build; the
viewer button that sends `RequestSecureAttention` stays hidden until they are.

1. **A helper that runs with system privileges.** It is installed once, with
   the user's consent, and starts the host in the active console session so a
   session can exist before anyone has signed in.
2. **Capturing the sign-in and lock screens.** The current capture path cannot
   see these protected desktops; a path that can is needed.
3. **Delivering Ctrl+Alt+Del** from `secure_attention::deliver()` on Windows.
4. **Where the unattended secret lives** when no user is signed in: a
   machine-wide store readable only by the helper, not the per-user keyring.

## Reference

RustDesk is AGPL, like OpenDesk, and implements all of the above. Its Windows
platform and service code are the practical reference for a contributor filling
in the parts listed here.
