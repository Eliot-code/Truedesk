---
title: Connecting and the session toolbar
description: What each button in the OpenDesk session toolbar does, how file transfer and recording work, and what Direct and Relayed mean.
---

Once the host accepts, the viewer shows the remote screen with a toolbar
across the top. From left to right:

| Control | What it does |
| --- | --- |
| Dot and ID | The host you're connected to. The dot turns amber while reconnecting. |
| **Direct** / **Relayed** badge | How the video travels, and the round trip in milliseconds. See [below](#direct-or-relayed). |
| **Fullscreen** | Fills the screen with the remote desktop. Press it again to leave. |
| Monitor picker | Lists the host's displays with their resolution and marks the primary. **Add display…** adds a [virtual display](/docs/virtual-displays/) when the host allows it. |
| **New window** | Opens the chosen monitor in its own window, so you can put each of the host's screens on one of yours. Shows when the host has more than one display. The window controls that monitor while your pointer is in it. |
| **Clipboard** | Turns clipboard sync on or off. It starts off. |
| **Sound** / **Muted** | Mutes or unmutes the host's sound on your side. |
| **Files** | Opens the file transfer panel. |
| **Record** / **Stop** | Records the session to a video file. A red timer shows while it runs. |
| **Restart** | Restarts the other computer and reconnects when it comes back. Shows only when the host has unattended access on and can restart on its own; it returns only if it is also set to start at sign-in. |
| **Privacy** | Blanks the other computer's screen and blocks its local keyboard and mouse, so nobody at it can watch or interfere while you work. You still see and control it normally. The host always restores both when the session ends, and someone at the host can regain control with Ctrl+Alt+Del. Windows only for now. |
| Timer | How long this session has run. |
| **Disconnect** | Ends the session. |

An **Encrypted** label sits in the bottom-right corner for the whole session.

The host sees a [control panel](/docs/host-controls/) instead: who is
connected, switches for what they're allowed to do, a log of what's happened,
and a button to disconnect them.

## Direct or Relayed

OpenDesk first tries a direct UDP connection between the two computers, the
same way video calls do. If that works, the badge says **Direct** and video
and input go straight between the two machines.

If a firewall or network blocks it, the badge says **Relayed**: the traffic
goes through the relay server instead. It's still end-to-end encrypted, just
usually slower. Hover over the badge for an explanation.

The number after the badge is the round trip time. It turns amber above
100 ms. On a relayed connection, a relay closer to both computers helps; see
[Self-hosting a relay](/docs/self-hosting/).

## Clipboard

With **Clipboard** on, text you copy on one side can be pasted on the other.
Only text is synced, up to 1 MB at a time. Either side can turn it off, and
the setting is remembered for the next session.

## Sound

The host sends its system sound (what you'd hear from its speakers), never its
microphone. That works on Windows, on macOS 14.6 and later, and on Linux with
PulseAudio or PipeWire (through pipewire-pulse). Other hosts send no sound.

## Files

Click **Files**, then **Send file…**, or drop files anywhere on the window.
Either side can send.

The other side is asked first: the host sees an **Accept this file?** dialog
with the name and size, and the viewer sees the offer in its file panel.
Accepted files are saved to the **Downloads** folder. OpenDesk checks each
file's SHA-256 hash before it finishes saving it, and gives a file a new name
rather than overwrite one that's already there.

## Recording

**Record** saves what you see of the remote screen as a `.webm` video in your
**Downloads** folder. Click **Stop** to finish. When the file is complete,
the app says *Recording saved to* and the path.

Recording happens on the viewer only. The host isn't told about it, so ask
before you record someone else's screen.

## When the connection drops

If the network drops during a session, the viewer shows *Reconnecting* and
tries to get back in by itself for a short while, with the same password. If
the host uses a [2FA code](/docs/unattended/#two-factor-codes), a reconnect
needs a fresh code, because each code can only be used once.

When the host clicks **Disconnect**, the viewer shows *The host ended the
session.* and doesn't reconnect.
