---
title: Troubleshooting and FAQ
description: Fixes for wrong passwords, offline devices, macOS permissions, Linux keyrings, relay errors and other common OpenDesk problems.
---

## "Wrong password"

> Wrong password. The host's password changes after every session, so check
> the current one and connect again.

The session password is one-time. The host makes a new one after every
session, and again after three wrong attempts in a row. Read the current one
off the host's home screen (**Show**) and try again.

If the host has unattended access on, use the **unattended** password
instead; the session password doesn't work then.

You may also see *The other device hung up while checking the password*. It
means the same thing.

## "That device is offline or already in a session"

The relay doesn't have a host with that ID right now. Check that:

- the ID is right (9 digits);
- OpenDesk is open on the other computer and its top bar says **Online**
  (if it says **Offline**, click **Start hosting**);
- both computers use the same relay (**RELAY REGIONS** on the home screen);
- nobody else is connected to it. A host takes one session at a time.

## "Nobody answered the connection request"

The host showed the **Allow remote control?** dialog, but nobody clicked
anything in time. Ask someone at the host to watch for it, or set up
[unattended access](/docs/unattended/).

## 2FA code errors

- *The host requires a two-factor code*: fill in **2FA code** with the
  current code from your authenticator app.
- *The 2FA code was rejected (wrong, expired, or already used)*: codes change
  every 30 seconds and work once. Wait for a fresh one. Also check that the
  clocks on both computers are right.
- *Reconnect needs a fresh 2FA code*: after a network drop the viewer can't
  reuse the code it already sent. Connect again with a new one.

## "The other device can't share its screen"

The host accepted, but it couldn't capture its screen. On the host:

- **No display.** A computer with no monitor connected (or with the display
  off) may have nothing to capture. Connect a monitor or an HDMI dummy plug,
  or turn on [virtual displays](/docs/virtual-displays/).
- **macOS**: check that **Screen Recording** is allowed for OpenDesk, then
  restart OpenDesk. See the next section if it already looks allowed.
- **Linux**: approve the screen-sharing request from the desktop portal when
  it appears.

Then connect again.

## macOS permissions stopped working after an update

The release builds aren't signed with an Apple Developer ID, and macOS ties
Screen Recording and Accessibility to the exact build. After an update or
reinstall, the switches in System Settings can still look on but no longer
apply. The **Permissions** screen opens at launch when that happens.

1. In **System Settings → Privacy & Security → Screen Recording**, select
   OpenDesk and remove it with **−**.
2. Add it again with **+** (it's in Applications) and turn it on.
3. Do the same under **Accessibility**.
4. Quit and reopen OpenDesk.

If that doesn't stick, reset both in Terminal and grant them again:

```sh
tccutil reset ScreenCapture com.opendesk.app
tccutil reset Accessibility com.opendesk.app
```

## Unattended access stopped working

The **UNATTENDED ACCESS** card shows a message when OpenDesk can't read its
password or 2FA secret from the system keyring. Until it's fixed, every
connection needs approval.

- *The unattended access password is missing from the system keyring*:
  enter the password again and click **Save settings**.
- *The 2FA secret is missing*: untick **Require 2FA code** and set it up
  again.
- **macOS**: unlock your login keychain in Keychain Access.
- **Windows**: make sure Credential Manager is available.
- **Linux**: install and unlock a keyring such as GNOME Keyring or KWallet.
  Minimal desktops and some window managers don't start one, and OpenDesk
  won't save the password in plain text instead.

## The relay refuses this computer

These show on the host's home screen.

- *This relay is limiting new devices from your network*: a relay lets one
  network claim 20 new IDs an hour. OpenDesk keeps trying; it can take up to
  an hour. Your own relay avoids it.
- *This relay is full and can't take new devices*: add another relay or
  [run your own](/docs/self-hosting/).
- *This relay is busy or limiting connections from your network*: OpenDesk
  keeps trying by itself.
- *This relay didn't accept this device's registration*: update OpenDesk and
  check the relay address.
- *Another computer is online with ID … on …*: two computers share one ID,
  usually because one is a copy of the other (a cloned disk or virtual
  machine). Click **Get a new ID** on one of them.
- *ID … belongs to another device on …*: this computer was offline long
  enough for the relay to give its ID away, or the ID was taken there. Click
  **Get a new ID**. Anyone who saved the old ID needs the new one.

When connecting, the viewer may show *The relay is limiting connections from
your network* (wait a minute and try again) or *The relay didn't accept the
connection request* (check the ID, and update OpenDesk if it keeps
happening).

## The badge says Relayed

A direct connection needs outbound UDP. Some corporate, hotel and mobile
networks block it, and then the session goes through the relay. It's still
end-to-end encrypted, only slower. The [browser viewer](/docs/browser-viewer/)
is always relayed.

## No sound

Sound needs Windows, macOS 14.6 or later (allow **System Audio Recording**
when asked), or Linux with PulseAudio or PipeWire. Check that **Sound** in
the toolbar isn't set to **Muted**. The browser viewer has no sound.

## Where are the logs?

Include the log when you [report a problem](https://github.com/treeot/opendesk/issues/new?template=bug_report.yml),
along with both OS versions and what you did:

- macOS: `~/Library/Logs/com.opendesk.app/`
- Windows: `%LOCALAPPDATA%\com.opendesk.app\logs\`
- Linux: `~/.local/share/com.opendesk.app/logs/`

## FAQ

### Do I need to open ports or set up port forwarding?

No. Both computers connect out to the relay on port 443. Only the relay
server needs ports 80 and 443 open.

### Can the person running the relay watch my session?

No. They can see which IDs connect, when, from which addresses, and how much
data flows, but not the screen, input, sound, files or clipboard. See the
[security model](/docs/security/).

### Does it work without a relay?

No. Even a direct connection starts at the relay, which pairs the two
computers. You can run your own.

### Is there a mobile app?

No. There are desktop apps for Windows, macOS and Linux, and the
[browser viewer](/docs/browser-viewer/).

### Why does Windows or macOS warn me about the app?

The builds aren't code-signed, so the OS can't tell who published them.
[Getting started](/docs/getting-started/#install) shows how to open
them.
