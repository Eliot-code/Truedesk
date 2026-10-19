---
title: Wake-on-LAN
description: Turn on a sleeping or switched-off computer from Truedesk, on the same network or through another computer that is on.
---

Truedesk can turn on a computer that is asleep or switched off by sending it a
Wake-on-LAN signal (a "magic packet"). It learns how to wake each computer
while you're connected to it, so connect once while it's on.

## Wake a saved device

On the home screen, a saved device that is offline shows a **Wake** button
once Truedesk knows how to wake it. Click it, wait a minute, and connect when
its dot turns green.

That works when your computer is on the same network as the sleeping one.
To wake a computer at another place, wake it through a computer that is on
there:

1. Connect to any computer running Truedesk on the sleeping computer's
   network.
2. Click **Wake** in the session toolbar.
3. Click **Wake** next to the computer to turn on.

Both your computer and the one you're connected to send the signal. The
computer you're connected to only sends it on its own networks, and only
while its **Control** permission is on.

## Set up the computer to wake

Wake-on-LAN has to be on in the computer itself. Wired connections work much
more reliably than Wi-Fi.

- **BIOS or UEFI**: turn on the setting called *Wake on LAN*, *Power on by
  PCI-E* or similar. Some computers also need *ErP* or *deep sleep* turned
  off so the network card keeps power.
- **Windows**: in Device Manager, open the network adapter, then on the
  **Power Management** tab check **Allow this device to wake the computer**
  and **Only allow a magic packet to wake the computer**. On the
  **Advanced** tab, set **Wake on Magic Packet** to Enabled. Turning off
  **Fast startup** helps a computer that is shut down (not just asleep) to
  wake.
- **Linux**: check with `sudo ethtool eth0 | grep Wake-on` (use your card's
  name). `g` means it's on; `sudo ethtool -s eth0 wol g` turns it on until
  the next restart, and NetworkManager keeps it with
  `nmcli connection modify <name> 802-3-ethernet.wake-on-lan magic`.

Truedesk learns the addresses of the computer's wired and wireless cards, not
of virtual ones. macOS hosts don't report them yet.

## When nothing happens

- Wait a full minute: a computer can take a while to start and connect to
  the relay.
- Make sure the sleeping computer is plugged in and its network cable is
  connected.
- Wake it through a computer on the same network: routers usually don't pass
  the signal from one network to another.
