import { useCallback, useEffect, useState } from "react";
import * as api from "../lib/api";
import { friendlyError } from "../lib/notify";

/** Why the other computer refused a tunnel connection. */
export function tunnelRefusal(code: string): string {
  switch (code) {
    case "not_allowed": return "The other computer doesn't allow tunnels.";
    case "unreachable": return "The other computer couldn't reach that address. Check the host and port.";
    case "limit": return "Too many tunnel connections are open. Close some and try again.";
    case "invalid": return "That host or port isn't valid.";
    case "no_answer": return "The other computer didn't answer the tunnel request.";
    default: return `The other computer refused the tunnel (${code}).`;
  }
}

/** A port from a form field: 1 to 65535, `0` for an empty optional field, or `undefined` when invalid. */
export function parsePort(value: string, optional = false): number | undefined {
  const trimmed = value.trim();
  if (trimmed === "") return optional ? 0 : undefined;
  if (!/^\d{1,5}$/.test(trimmed)) return undefined;
  const port = Number(trimmed);
  return port >= 1 && port <= 65535 ? port : undefined;
}

/** `host:port`, with an IPv6 address in brackets. */
export function displayTarget(host: string, port: number): string {
  return host.includes(":") ? `[${host}]:${port}` : `${host}:${port}`;
}

export interface TunnelPanelProps {
  open: boolean;
  onClose: () => void;
}

/**
 * TCP tunnels: a port on this computer that leads to an address on the other
 * computer's network. Stays mounted for the session so the list stays current.
 */
export default function TunnelPanel({ open, onClose }: TunnelPanelProps) {
  const [tunnels, setTunnels] = useState<api.TunnelInfo[]>([]);
  const [localPort, setLocalPort] = useState("");
  const [host, setHost] = useState("localhost");
  const [port, setPort] = useState("");
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(() => { void api.listTunnels().then(setTunnels).catch(() => {}); }, []);

  useEffect(() => {
    refresh();
    const listener = api.onTunnelsChanged(refresh);
    return () => { void listener.then((stop) => stop()); };
  }, [refresh]);

  const add = async () => {
    const target = host.trim();
    const remotePort = parsePort(port);
    const local = parsePort(localPort, true);
    if (!target) { setError("Enter the address to reach from the other computer, like localhost or 192.168.1.20."); return; }
    if (remotePort === undefined) { setError("Enter the remote port, from 1 to 65535."); return; }
    if (local === undefined) { setError("The local port must be from 1 to 65535, or empty to pick a free one."); return; }
    setBusy(true);
    setError(undefined);
    try {
      await api.openTunnel(local, target, remotePort);
      setLocalPort("");
      setPort("");
      refresh();
    } catch (failure) {
      setError(friendlyError(failure));
    } finally {
      setBusy(false);
    }
  };

  const close = (id: number) => {
    void api.closeTunnel(id).then(refresh).catch((failure: unknown) => setError(friendlyError(failure)));
  };

  return (
    <section className="tunnel-panel" aria-label="TCP tunnels" hidden={!open}>
      <div className="tunnel-panel__header"><span className="eyebrow">TCP TUNNELS</span><button className="icon-button tunnel-panel__close" type="button" onClick={onClose} aria-label="Close tunnels">×</button></div>
      <p className="tunnel-panel__hint">Reach a service on the other computer's network, like Remote Desktop (3389), SSH (22) or a web page, through this encrypted session. Then point your app at the local address.</p>
      {tunnels.length > 0
        ? <ul className="tunnel-panel__list">
            {tunnels.map((tunnel) => (
              <li className="tunnel-panel__item" key={tunnel.id}>
                <span><span className="mono">localhost:{tunnel.local_port}</span> → <span className="mono">{displayTarget(tunnel.host, tunnel.port)}</span></span>
                <button className="ghost-button" type="button" onClick={() => close(tunnel.id)} aria-label={`Close tunnel on port ${tunnel.local_port}`}>Close</button>
                <small>{tunnel.connections === 1 ? "1 connection" : `${tunnel.connections} connections`}</small>
              </li>
            ))}
          </ul>
        : <p className="tunnel-panel__empty">No tunnels yet.</p>}
      <form className="tunnel-panel__form" onSubmit={(event) => { event.preventDefault(); void add(); }}>
        <label>Local port<input inputMode="numeric" placeholder="Auto" value={localPort} onChange={(event) => setLocalPort(event.target.value)} /></label>
        <label>Remote host<input autoCapitalize="off" autoCorrect="off" spellCheck={false} value={host} onChange={(event) => setHost(event.target.value)} /></label>
        <label>Remote port<input inputMode="numeric" placeholder="3389" value={port} onChange={(event) => setPort(event.target.value)} /></label>
        <button className="primary-button" type="submit" disabled={busy}>Add tunnel</button>
      </form>
      {error && <p className="tunnel-panel__error" role="alert">{error}</p>}
    </section>
  );
}
