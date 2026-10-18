Beta build for testing. Expect rough edges, and please report what breaks.

## What's new

- **Remote file manager (new):** the Files panel now has a two-pane browser,
  your computer on the left and the other one on the right. Open folders on
  both, and download a remote file with the arrow next to it (it saves to your
  Downloads). "Send file…" still sends the other way.
- **Address book shows who's online:** saved devices now have a green dot when
  they're reachable, and online ones sort to the top.
- **Restart the remote and reconnect:** when the other computer has unattended
  access on, a **Restart** button reboots it and reconnects automatically once
  it's back. Windows only for now.
- **Privacy mode (Windows, new):** blank the other computer's screen and block
  its local keyboard and mouse while you work; you still see and control it
  normally. It always restores when the session ends, and someone at that
  computer can take back control with Ctrl+Alt+Del.
- These are new; please report anything rough, especially on Windows.

If you installed an earlier beta, this one arrives as an "Update available"
banner on the home screen. On macOS you may need to grant Screen Recording and
Accessibility again after updating.

## Install

Pick the file for your system below. These builds are not code-signed, so
your OS will warn the first time you open them:

- **macOS** (`.dmg`, Apple silicon, macOS 12.3+): drag Truedesk to
  Applications. The first time, right-click it and choose **Open** (or allow
  it in System Settings → Privacy & Security → **Open Anyway**). If macOS
  says it's damaged, run
  `xattr -dr com.apple.quarantine /Applications/Truedesk.app` and open it
  again. Grant **Screen Recording** and **Accessibility** when asked, then
  restart the app.
- **Windows** (`.exe` or `.msi`): on "Windows protected your PC", click
  **More info → Run anyway**.
- **Linux** (`.AppImage`, `.deb`, or `.rpm`): for the AppImage,
  `chmod +x Truedesk*.AppImage` and run it.

The app connects to the project's test relay out of the box. Nothing needs
port forwarding.

## Try it

1. Install on two computers and open Truedesk on both.
2. On the one you want to control, note the **ID** and **session password**.
3. On the other, enter them and click **Connect**, then accept on the first.

The full checklist is in
[docs/TESTING.md](https://github.com/Eliot-code/truedesk/blob/main/docs/TESTING.md).

## Needs testing

These features are new and haven't run on real hardware yet:

- **Windows:** hardware video encoding (NVIDIA / Intel / AMD). Check the log
  for `video encoder:` to see which encoder was picked.
- **Linux:** hearing the host's system sound (PulseAudio / PipeWire).
- **macOS:** system sound needs macOS 14.6+; older versions send no audio.

## Report a problem

[Open an issue](https://github.com/Eliot-code/truedesk/issues/new?template=bug_report.yml)
with your OS versions, what you did, and the app log
(macOS `~/Library/Logs/com.truedesk.app/`, Windows
`%LOCALAPPDATA%\com.truedesk.app\logs\`, Linux
`~/.local/share/com.truedesk.app/logs/`).
Report security problems privately (see
[SECURITY.md](https://github.com/Eliot-code/truedesk/blob/main/docs/SECURITY.md)).
