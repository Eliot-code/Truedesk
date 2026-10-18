---
title: Updates
description: How Truedesk checks for, downloads and installs new versions.
---

Truedesk checks for a new version when it starts and every six hours after
that. When there is one, a banner appears in the corner of the home screen:

> Update available: v0.1.0-beta.6 · **Install and restart**

Click **Install and restart**. The app downloads the update, checks its
signature against the project's update key, installs it and opens again. You
can **Cancel** while it downloads, or dismiss the banner with **×** to skip
that version until the app restarts.

The banner never appears during a session. If an update finishes downloading
while you're connected, it waits and offers **Restart to update** once the
session ends. If something fails, the banner says *The update didn't install*
and offers **Try again**.

Update both computers. Most changes affect both sides of a session.

## Per platform

- **macOS** replaces `/Applications/Truedesk.app` in place. Because the
  builds aren't signed with an Apple Developer ID, macOS may forget the
  Screen Recording and Accessibility permissions after an update. The
  **Permissions** screen opens at launch when that happens; see
  [Troubleshooting](/docs/troubleshooting/#macos-permissions-stopped-working-after-an-update).
- **Windows** runs the same kind of installer you used (`.exe` or `.msi`),
  showing a progress bar, then restarts the app.
- **Linux**: the AppImage updates itself. For `.deb` and `.rpm` installs the
  system asks for an administrator password.

## Older versions

Versions up to v0.1.0-beta.4 don't have the updater. Install a newer release
by hand once; it updates itself from then on. The releases, with their
changelogs, are on the [releases page](https://github.com/Eliot-code/truedesk/releases).
