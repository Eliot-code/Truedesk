---
title: Host controls and permissions
description: While someone controls your computer, choose what they can do, watch what happens, and disconnect them without stopping hosting.
---

While a viewer is connected, the host shows a control panel for the person
sitting at the shared computer. It lists who is connected, what they're
allowed to do, and a log of what has happened, and it has a button to end the
session.

## Permissions

Five switches decide what the connected viewer can do. All five start **on**
when a session begins, and turning one off takes effect at once.

| Permission | Off means |
| --- | --- |
| **Control** | The viewer can see the screen but their mouse and keyboard do nothing. |
| **File transfer** | Files the viewer sends are refused without a prompt. |
| **Clipboard** | Clipboard text stops syncing in both directions. |
| **Audio** | The host stops sending its sound. |
| **TCP tunnels** | The viewer can't open [tunnels](/docs/tunnels/) to this computer's network, and the open ones close. |

These are enforced by the host, not the viewer. When **Control** is off, the
viewer's input never reaches your computer; when **Audio** is off, no sound is
sent. A viewer cannot turn any of them back on.

## Activity log

The panel keeps a short log of the session: when the viewer connected and
disconnected, each file received or refused, each tunnel connection and the
address it reached, and every permission change. It clears when a new
session starts.

A viewer can only ever **send** you a file, and you're asked to accept each
one; there is no way for a viewer to take a file from your computer. See
[Files](/docs/sessions/#files).

## Disconnect

**Disconnect viewer** ends the current session and tells the viewer the host
ended it. It does not stop hosting: your computer stays reachable, and the
next viewer can connect with the usual password. To stop being reachable at
all, stop hosting on the home screen.
