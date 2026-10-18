---
title: TCP tunnels
description: Reach Remote Desktop, SSH, a web page or any TCP service on the other computer's network through the encrypted Truedesk session.
---

A TCP tunnel is a port on your computer that leads to a service on the other
computer's side. Use it to reach Remote Desktop (port 3389), SSH (22), a
router's web page, a database or anything else that speaks TCP, either on the
other computer itself or on another machine in its network. The traffic goes
through the session, so it is end-to-end encrypted like everything else and
needs no port forwarding on either network.

## Open a tunnel

1. Connect to the other computer as usual.
2. Click **Tunnels** in the toolbar. It shows when the other computer's
   Truedesk version supports tunnels.
3. Fill in:
   - **Local port**: the port on your computer. Leave it empty to pick a free
     one.
   - **Remote host**: where the other computer should connect.
     `localhost` means the other computer itself; an address like
     `192.168.1.20` or a name like `printer.local` reaches another machine
     in its network.
   - **Remote port**: the service's port, like `3389` for Remote Desktop.
4. Click **Add tunnel**. Then point your app at `localhost:` and the local
   port shown in the list, for example `localhost:50123` in Remote Desktop
   Connection, or `ssh -p 50123 user@localhost`.

Each tunnel shows how many connections are using it. **Close** stops
listening and ends its connections. Tunnels end with the session; after a
reconnect, open them again.

The local port only listens on `127.0.0.1`, so other computers in your
network can't use your tunnels.

## On the host

The person at the other computer sees every tunnel connection in the
[activity log](/docs/host-controls/#activity-log), with the address it
reached. Turning off **TCP tunnels** in the
[host controls](/docs/host-controls/#permissions) refuses new tunnel
connections and ends the open ones at once.

## When it doesn't work

| Message | What to check |
| --- | --- |
| The other computer doesn't allow tunnels. | The host turned off **TCP tunnels** for this session. |
| The other computer couldn't reach that address. | The service isn't running, a firewall on that side blocks it, or the host or port is wrong. Try it from the other computer itself first. |
| Port … is already in use on this computer. | Another program uses that local port. Pick another one or leave it empty. |
| The other computer didn't answer the tunnel request. | The session may be reconnecting. Try again in a moment. |
