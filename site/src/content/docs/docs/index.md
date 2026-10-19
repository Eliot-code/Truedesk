---
title: Truedesk documentation
description: How to install Truedesk, connect to another computer, run your own relay, and what the encryption does and doesn't protect.
---

Truedesk is a remote desktop app. One computer **hosts** (shares its screen and
accepts input), the other **views** (sees that screen and controls it). The
same app does both. The two computers find each other through a **relay**, a
small server that you can run yourself, and encrypt everything between
themselves so the relay can't read it.

Truedesk is in beta. The builds aren't code-signed, and Windows and Linux
hosting has had less testing than macOS.

## Start here

- [Getting started](/docs/getting-started/): install the app on Windows, macOS
  or Linux, get past the unsigned-app warnings, and make a first connection.
- [Connecting and the session toolbar](/docs/sessions/): what each toolbar
  button does, file transfer, recording, and what Direct and Relayed mean.
- [Unattended access and 2FA](/docs/unattended/): reach your own computer
  when nobody is there to click Allow.
- [TCP tunnels](/docs/tunnels/): reach Remote Desktop, SSH or a web page on
  the other computer's network through the session.
- [Wake-on-LAN](/docs/wake-on-lan/): turn on a sleeping computer, nearby or
  through another computer on its network.
- [Browser viewer](/docs/browser-viewer/): control a computer from a browser,
  and what it can't do yet.
- [Phones and tablets](/docs/mobile/): the Android app, iPhone through
  Safari, touch gestures and typing.

## Running and trusting it

- [Self-hosting a relay](/docs/self-hosting/): Docker Compose with automatic
  HTTPS, the prebuilt image, and the settings.
- [Security model](/docs/security/): what is encrypted, what the relay can
  still see, and the known limits.
- [Updates](/docs/updates/): how the app updates itself.
- [Troubleshooting and FAQ](/docs/troubleshooting/): wrong passwords, offline
  devices, macOS permissions, Linux keyrings and relay errors.

## Words used in these docs

| Term | Meaning |
| --- | --- |
| Host | The computer being viewed and controlled. |
| Viewer | The computer (or browser) doing the controlling. |
| ID | A 9-digit number that identifies a host, like `482 913 075`. |
| Session password | A one-time password shown on the host. It changes after every session. |
| Relay | The server both sides connect to. It forwards encrypted data and can't decrypt it. |

Something missing or wrong? [Open an issue](https://github.com/Eliot-code/truedesk/issues)
or use the edit link at the bottom of any page.
