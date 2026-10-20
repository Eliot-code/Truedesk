---
title: Browser viewer
description: Control a computer running Truedesk from a web browser, without installing anything. What it can do, what it can't, and what you're trusting.
---

The [browser viewer](/connect/) lets you control a computer from a browser,
for example on a borrowed laptop where you can't install anything. The
computer you connect to still needs the Truedesk app to host.

## Connect

1. Open `/connect/` on your Truedesk website.
2. Enter the **Device ID** and **Password** shown in Truedesk on the other
   computer. If it asks for one, add the **2FA code**.
3. Click **Connect**, and wait for the other side to click **Allow** (unless
   it has [unattended access](/docs/unattended/) on).

The page shows its own **YOUR ID** at the top. That's the ID the host sees
in its approval dialog. The browser keeps it, so the host sees the same ID
next time.

Under **Relay** you can change which relay the page uses. It has to be the
same relay the host uses, and it must start with `wss://`.

### Invite link

You can share a link that fills the ID in for whoever opens it:
`https://your-truedesk-site/connect/?id=123456789`. Add `&relay=wss://…`
to point them at the right relay too. The link never carries the password,
so you still tell them that (or turn on unattended access).

## What works

- The remote screen, mouse and keyboard.
- The host's **sound**.
- **File transfer**, both ways, with a prompt before anything is accepted.
- **Clipboard** text sync, when you turn it on.
- **Recording** the remote screen to a `.webm` file.
- **Fullscreen** and choosing a monitor when the host has more than one.
- Reconnecting after a short network drop.

## What doesn't, yet

- **Always relayed.** Browsers can't join the direct UDP path, so the session
  always goes through the relay. It's still end-to-end encrypted, but
  usually slower than the app's direct connection.
- **It can't host.** You can control a computer from the browser, not share
  the browser's own screen.

It needs a browser with WebCodecs: a recent Chrome, Edge, Firefox or Safari.
In a browser without it, the page says so and the **Connect** button stays
off.

## What you're trusting

The encryption is the same Rust code the desktop app uses, compiled to
WebAssembly, so the relay still sees only sealed bytes.

The difference is where that code comes from. GitHub Pages serves the page
fresh on every visit, so each time you load it you're trusting GitHub and
this repository, not a binary you built or checked once. If that matters for
your setup, use the desktop app, or build the browser viewer yourself and
host it; the [README](https://github.com/Eliot-code/truedesk#browser-viewer)
explains how.
