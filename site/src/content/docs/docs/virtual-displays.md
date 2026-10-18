---
title: Virtual displays
description: Add a screen that exists only in software, and reach a computer that has no monitor plugged in.
---

A virtual display is a screen that exists only in software. The host treats
it like a real monitor, so you can stream it and control it as usual. Two
things use it:

- **A second screen from the viewer.** In a session, open the monitor menu and
  choose **Add display…**. The host adds a screen sized to your window and
  switches to it. **Remove display** takes it away again.
- **Computers with no monitor.** A Mac mini in a cupboard or a PC without a
  screen often has nothing to capture. With virtual displays allowed, the host
  adds a 1920x1080 screen for the session when you connect.

Every virtual display Truedesk adds is removed when the session ends or the
app quits. A viewer can add up to two per session.

## Allow it on the host

On the host, under hosting settings on Home, tick **Let viewers add virtual
displays**. It's on by default. The person at the host gets a notice each
time one is added.

## macOS

Nothing to install. Truedesk uses the virtual display support built into
macOS.

With HiDPI the size you ask for is in points, so a display added from a Retina
window may show at half its pixel size. Text stays readable either way.

## Windows

Windows can't add a screen without a display driver. Truedesk uses the free
**USB Mobile Monitor** driver by Amyuni, which is signed by Microsoft and is the
same driver RustDesk uses.

1. On the host, with virtual displays allowed, click **Install virtual
   display driver**.
2. Read Amyuni's licence and click **Accept and install**.
3. Approve the administrator prompt.

Truedesk downloads the driver from Amyuni, checks the file against a fixed
fingerprint before using it, and installs it with Windows' own driver tools.
The button changes to **Uninstall** once the driver is in.

Limits of the driver:

- Sizes snap to a fixed list: 1280x720, 1366x768, 1600x900, 1920x1080,
  1920x1200, 2560x1440, 2560x1600, 2880x1800, 3440x1440 and 3840x2160. The
  monitor menu shows the size you actually got.
- At most four virtual screens on the computer, counting ones added by other
  apps.
- Displays come off newest first. If you try to remove an older one, Truedesk
  asks you to remove the newer one first.
- Any program on that computer can add or remove the driver's screens. See
  [Security model](/docs/security/).

## Linux

Not supported yet. Connect a monitor, or an HDMI dummy plug (a small adapter
that pretends to be a monitor).

## Without a virtual display

If the host has no screen and virtual displays are off or unavailable, the
viewer sees "That computer has no screen connected" with the options above. An
HDMI dummy plug works on every system and needs no setup.
