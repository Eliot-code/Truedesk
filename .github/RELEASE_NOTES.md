**Beta build for testing.** Expect rough edges, and please report what breaks.

## Install

Pick the file for your system below. These builds are **not code-signed**, so
your OS will warn the first time you open them:

- **macOS** (`.dmg`): drag OpenDesk to Applications. If macOS says it's
  damaged or can't be verified, run
  `xattr -dr com.apple.quarantine /Applications/OpenDesk.app`, then open it.
  Grant **Screen Recording** and **Accessibility** when asked, then restart
  the app.
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

## Report a problem

[Open an issue](https://github.com/treeot/opendesk/issues/new?template=bug_report.yml)
with your OS versions, what you did, and the app log
(macOS `~/Library/Logs/com.opendesk.app/`, Windows
`%LOCALAPPDATA%\com.opendesk.app\logs\`, Linux
`~/.local/share/com.opendesk.app/logs/`).
Security problems: please report privately (see
[SECURITY.md](https://github.com/treeot/opendesk/blob/main/docs/SECURITY.md)).
