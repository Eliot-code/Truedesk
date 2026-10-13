---
title: Unattended access and 2FA
description: Reach your own computer without anyone clicking Allow, protect it with an authenticator app code, and keep it reachable after a restart.
---

Normally someone has to sit at the host, read out the session password and
click **Allow**. Unattended access is for your own computers, where nobody is
there to do that: you set a fixed password, and anyone who knows it connects
without a prompt.

:::caution
Anyone with the unattended password can control the computer. Use a long,
unique password, turn on a 2FA code, and turn unattended access off when you
don't need it.
:::

## Turn it on

On the host, find the **UNATTENDED ACCESS** card (*Connect without
approval*).

1. Type a password of at least 12 characters in **Set a password**.
2. Tick **Enabled**.
3. Click **Save settings**. Nothing changes until you save.

From then on, connect to this computer with the unattended password in the
**Password** field. While unattended access is on, the host doesn't use its
one-time session password.

To change the password later, type a new one in the same field and save. To
turn it off, untick **Enabled** and save; connections need approval again.

## Two-factor codes

A 2FA code means a stolen password alone isn't enough. It works with any
authenticator app that supports time-based codes (TOTP), such as the ones
built into password managers.

1. Turn on and save unattended access first.
2. Tick **Require 2FA code**. A QR code appears.
3. Scan it with your authenticator app, or type the setup key shown next to
   it. The account is labelled *OpenDesk* and the host's ID.
4. Enter the current **6-digit code** from the app and click **Confirm**.
   Two-factor is only on after this step; **Cancel** leaves it off.

When you connect, put the current code from the app in **2FA code
(optional)** next to the password. Each code works once, so an automatic
reconnect after a network drop asks for a new one.

If you lose the authenticator, untick **Require 2FA code** on the host and
set it up again.

## Start at sign-in

So that a restarted computer is reachable again, tick **Start OpenDesk when I
sign in** on the same card. It can only be turned on once unattended access
is saved and working.

With it on, OpenDesk starts hidden after you sign in, keeps hosting, and
shows an icon in the system tray (the menu bar on macOS). Its menu has
**Show OpenDesk**, **Start hosting** or **Stop hosting**, and **Quit
OpenDesk**. Closing the window leaves OpenDesk running in the tray; use
**Quit OpenDesk** to stop it.

This only runs after someone signs in to the computer. Nothing runs at the
login screen, and OpenDesk isn't installed as a system service. Under the
hood it's a per-user sign-in item: a LaunchAgent on macOS, the `Run` registry
key on Windows, an XDG autostart entry on Linux.

## Where the secrets are kept

The unattended password and the 2FA secret are stored in the OS keychain:
the login keychain on macOS, Credential Manager on Windows, and the Secret
Service keyring (GNOME Keyring, KWallet) on Linux. Anyone who can unlock your
user account can read them there. They're never written to disk in plain
text.

If OpenDesk can't read them (the keychain is locked, was reset, or the
settings were copied to a new machine), the card says so, each connection
falls back to manual approval, and a launch at sign-in opens the window
instead of hiding in the tray. See [Troubleshooting](/docs/troubleshooting/#unattended-access-stopped-working).
