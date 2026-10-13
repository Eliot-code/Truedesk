# Windows sign-in screen, UAC and Ctrl+Alt+Del

Reaching the Windows sign-in screen, UAC prompts and the lock screen needs a
small helper that runs with system privileges and launches the host inside the
active console session. RustDesk, TeamViewer and AnyDesk all work this way.
OpenDesk keeps this capability opt-in. The normal user host continues to run
in the signed-in desktop; the Windows service is installed only after the user
enables the setting and accepts the administrator prompt.

## What is in place (scaffold)

- `Control::RequestSecureAttention` — the viewer asks the host to deliver
  Ctrl+Alt+Del (`crates/protocol/src/control.rs`). Old peers ignore it.
- `host_core::secure_attention::deliver()` — called when the host receives
  that message (`crates/host-core/src/secure_attention.rs`). On the
  service-launched Windows host it calls the OS secure-attention broker.
- `send_secure_attention` — the Tauri command and the `sendSecureAttention`
  API binding the viewer UI calls.

## What is not implemented

The service setting is exposed in Home as “Run OpenDesk at the Windows sign-in
screen”. Its status is read from the Windows service manager, and the ordinary
host refuses to start while the service is running, displaying “Hosted by the
OpenDesk service”.

1. **A helper that runs with system privileges.** It is installed once, with
   the user's consent, as `OpenDeskSignIn` under `LocalSystem`. The service
   duplicates its SYSTEM token into the active console session and launches
   the same executable with an explicit service-host argument. The launch is
   marked explicitly so secure-desktop capture and secure attention are never
   enabled for an ordinary user launch.
2. **Capturing the sign-in and lock screens.** The service host selects the
   current input desktop on its capture thread and uses a GDI desktop DC. This
   is selected only for the helper-launched host; signed-in hosts continue to
   use Scap/Windows Graphics Capture.
3. **Delivering Ctrl+Alt+Del** through the Windows `SendSAS` broker from the
   service-launched process. The viewer control is advertised only in that
   mode.
4. **Where the unattended secret lives** when no user is signed in: enabling
   the service stages the keyring values, and the SYSTEM service promotes them
   into `%ProgramData%\OpenDesk\secrets.json`, protected to SYSTEM and local
   Administrators. The service-host process reads that store; normal launches
   continue using the per-user keyring.

## Manual verification

On a disposable Windows machine, enable unattended access and then enable the
sign-in-screen setting. Approve the administrator prompt, sign out, lock the
machine, and trigger a UAC prompt. Verify that a viewer can see each desktop,
move the pointer, type, and use Ctrl+Alt+Del. Disable the setting and confirm
that the service is removed and the ordinary signed-in host starts again.

## Reference

RustDesk is AGPL, like OpenDesk, and implements all of the above. Its Windows
platform and service code are the practical reference for a contributor filling
in the parts listed here.
