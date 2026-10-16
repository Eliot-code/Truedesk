Beta build for testing. Expect rough edges, and please report what breaks.

## What's new

- **Browser viewer, now with sound, files, clipboard and recording:** the
  viewer at /connect plays the host's audio, transfers files both ways, syncs
  the clipboard, and can record the session to a local video file.
- **2FA prompt:** if the host asks for a 2FA code, you now connect with just
  the ID and password and a popup asks for the code, the way AnyDesk does.
  The separate "2FA code (optional)" box is gone.
- Refreshed the app's icon and header mark.

If you installed an earlier beta, this one arrives as an "Update available"
banner on the home screen. On macOS you may need to grant Screen Recording and
Accessibility again after updating.

## Install

Pick the file for your system below. These builds are not code-signed, so
your OS will warn the first time you open them:

- **macOS** (`.dmg`, Apple silicon, macOS 12.3+): drag OpenDesk to
  Applications. The first time, right-click it and choose **Open** (or allow
  it in System Settings → Privacy & Security → **Open Anyway**). If macOS
  says it's damaged, run
  `xattr -dr com.apple.quarantine /Applications/OpenDesk.app` and open it
  again. Grant **Screen Recording** and **Accessibility** when asked, then
  restart the app.
- **Windows** (`.exe` or `.msi`): on "Windows protected your PC", click
  **More info → Run anyway**.
- **Linux** (`.AppImage`, `.deb`, or `.rpm`): for the AppImage,
  `chmod +x OpenDesk*.AppImage` and run it.

The app connects to the project's test relay out of the box. Nothing needs
port forwarding.

## Try it

1. Install on two computers and open OpenDesk on both.
2. On the one you want to control, note the **ID** and **session password**.
3. On the other, enter them and click **Connect**, then accept on the first.

The full checklist is in
[docs/TESTING.md](https://github.com/treeot/opendesk/blob/main/docs/TESTING.md).

## Needs testing

These features are new and haven't run on real hardware yet:

- **Windows:** hardware video encoding (NVIDIA / Intel / AMD). Check the log
  for `video encoder:` to see which encoder was picked.
- **Linux:** hearing the host's system sound (PulseAudio / PipeWire).
- **macOS:** system sound needs macOS 14.6+; older versions send no audio.

## Report a problem

[Open an issue](https://github.com/treeot/opendesk/issues/new?template=bug_report.yml)
with your OS versions, what you did, and the app log
(macOS `~/Library/Logs/com.opendesk.app/`, Windows
`%LOCALAPPDATA%\com.opendesk.app\logs\`, Linux
`~/.local/share/com.opendesk.app/logs/`).
Report security problems privately (see
[SECURITY.md](https://github.com/treeot/opendesk/blob/main/docs/SECURITY.md)).
