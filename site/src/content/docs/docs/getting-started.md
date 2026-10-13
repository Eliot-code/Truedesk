---
title: Getting started
description: Install OpenDesk on Windows, macOS or Linux, open an unsigned build, and connect to another computer for the first time.
---

You need OpenDesk on the computer you want to reach (the host). On the
computer you're sitting at, use the app too, or the
[browser viewer](/docs/browser-viewer/) if you can't install anything there.

Download the newest build from the
[releases page](https://github.com/treeot/opendesk/releases). Every build so
far is a beta, so GitHub lists them as pre-releases.

## Install

OpenDesk builds are not code-signed, so each OS warns you the first time you
open the app. The steps below get past that warning. Only do this for a file
you downloaded from the project's own releases page.

### macOS

The `.dmg` is for Apple silicon Macs running macOS 12.3 or later. There's no
Intel build yet.

1. Open the `.dmg` and drag **OpenDesk** to **Applications**.
2. Open OpenDesk. macOS says it can't verify the developer. Click **Done**
   (or **Cancel**).
3. Open **System Settings → Privacy & Security**, scroll down to the message
   about OpenDesk, and click **Open Anyway**. Confirm with your password.
   On older macOS versions you can instead Control-click the app in Finder and
   choose **Open**.
4. If macOS says the app **is damaged and can't be opened**, run this in
   Terminal, then open it again:

   ```sh
   xattr -dr com.apple.quarantine /Applications/OpenDesk.app
   ```

To share its screen and accept remote control, the Mac needs two permissions.
OpenDesk shows a **Permissions** screen at launch while either is missing:

- **Screen Recording**, so the other side can see the screen.
- **Accessibility**, so the other side can use the mouse and keyboard.

Click **Open Settings** next to each one, turn OpenDesk on in the list, and
restart OpenDesk. You can choose **Skip for now** if this Mac only controls
other computers; it doesn't need either permission for that. The
**Permissions** button on the home screen brings the screen back.

The first time a session plays sound, macOS 14.6 and later also ask to allow
**System Audio Recording**. Older macOS versions send no sound.

### Windows

Windows 10 (version 1803) or later, 64-bit. Use the `-setup.exe` installer,
or the `.msi` if you deploy software with Group Policy or similar tools.

1. Run the installer. Windows shows **Windows protected your PC**.
2. Click **More info**, then **Run anyway**.

### Linux

Builds are for x86_64. Pick one:

- **AppImage**: runs anywhere and updates itself.

  ```sh
  chmod +x OpenDesk_*.AppImage
  ./OpenDesk_*.AppImage
  ```

- **.deb** (Debian, Ubuntu): `sudo apt install ./OpenDesk_*_amd64.deb`
- **.rpm** (Fedora, openSUSE): `sudo dnf install ./OpenDesk-*.x86_64.rpm`

To host, Linux needs PipeWire and `xdg-desktop-portal`. When someone connects,
your desktop asks which screen to share; approve it. Remote input works best
on X11. On Wayland it's limited.

[Unattended access](/docs/unattended/) stores its password in the system
keyring, so it needs GNOME Keyring, KWallet or another Secret Service keyring.

## Connect for the first time

Open OpenDesk on both computers. Each one starts hosting as soon as it opens,
and the top bar shows **Online** once it has reached the relay.

**On the computer you want to reach (the host):**

1. Find **YOUR ID**, a 9-digit number, and the **SESSION PASSWORD** below it.
   Click **Show** to read the password, or the copy button next to it.
2. Give both to the person who will connect: read them out, or send them
   over a channel you trust.

**On the computer you're using (the viewer):**

1. Under **CONTROL A REMOTE DEVICE**, enter the ID and the password.
   Leave **2FA code (optional)** empty unless the host has
   [two-factor](/docs/unattended/#two-factor-codes) turned on.
2. Click **Connect**. The app shows *Waiting for … to accept*.

**Back on the host**, a dialog asks **Allow remote control?** It shows the
ID of the device asking and what it will be able to do: see your screen and
control the mouse and keyboard. Click **Allow** to start, or **Reject**.
Reject is selected by default, so pressing Enter by accident doesn't let
anyone in.

The viewer now shows the host's screen. To end the session, click
**Disconnect** on either side.

:::note
The session password changes after every session, and after repeated wrong
attempts. Always read the current one off the host. For a computer you
connect to regularly, set up [unattended access](/docs/unattended/) instead.
:::

### Save devices you use often

- **RECENT** under the connect form lists IDs you connected to lately. Click
  one to fill in the ID.
- The **ADDRESS BOOK** saves a device under a name. **Connect** fills in its
  ID; you still type the password.
- **ALLOWED DEVICES** (on the host) marks IDs as **Known device** in the
  approval dialog. They still need your approval and the password. Ticking
  **Always allow this device** in the dialog adds the ID to this list.

## Which relay is used

The app connects to the project's test relay out of the box. Both computers
need to use the same relay. To use your own, see
[Self-hosting a relay](/docs/self-hosting/) and enter its address under
**RELAY REGIONS** on the home screen.
