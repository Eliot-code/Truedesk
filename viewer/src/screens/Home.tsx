import { useEffect, useState, type FormEvent } from "react";
import * as api from "../lib/api";

interface HomeProps { status: api.Status; incomingRequest: api.IncomingRequest | null; onDismissRequest: () => void }
const statusLabels: Record<api.Status, string> = { offline: "Offline", online: "Online", connecting: "Connecting", in_session: "In session" };

export default function Home({ status, incomingRequest, onDismissRequest }: HomeProps) {
  const [identity, setIdentity] = useState<api.Identity | null>(null);
  const [recentIds, setRecentIds] = useState<string[]>([]);
  const [targetId, setTargetId] = useState("");
  const [remotePassword, setRemotePassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);

  useEffect(() => {
    let mounted = true;
    void Promise.all([api.getIdentity(), api.getConfig()]).then(([loadedIdentity, config]) => {
      if (mounted) { setIdentity(loadedIdentity); setRecentIds(config.recent_ids); }
    }).catch((error: unknown) => mounted && setMessage(String(error)));
    void api.startHosting().catch((error: unknown) => mounted && setMessage(String(error)));
    return () => { mounted = false; };
  }, []);

  const rotate = async () => {
    try { const result = await api.rotatePassword(); setIdentity((current) => current ? { ...current, password: result.password } : current); }
    catch (error) { setMessage(String(error)); }
  };
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!/^\d{9}$/.test(targetId)) { setMessage("Enter a 9-digit device ID."); return; }
    if (!remotePassword) { setMessage("Enter the remote device password."); return; }
    setBusy(true); setMessage(null);
    try { await api.connect(targetId, remotePassword); } catch (error) { setMessage(String(error)); } finally { setBusy(false); }
  };
  const copyId = async () => {
    if (!identity) return;
    try { await navigator.clipboard.writeText(identity.id); setMessage("ID copied."); } catch { setMessage("Could not copy ID."); }
  };

  return <main className="app-shell">
    <header className="topbar"><div className="brand"><span className="lock-glyph" aria-hidden="true">⌑</span> OpenDesk</div><div className="status" data-status={status}><span className="status-dot" aria-hidden="true" /><span>{statusLabels[status]}</span></div></header>
    <section className="hero"><span className="eyebrow">PERSONAL DEVICE ACCESS</span><h1>Welcome back.</h1><p>Your device is ready when you are.</p></section>
    <div className="cards">
      <section className="card identity-card"><span className="eyebrow">YOUR ID</span><div className="id-row"><strong className="mono identity-value">{identity?.id ?? "—"}</strong><button className="icon-button" type="button" onClick={() => void copyId()} aria-label="Copy device ID">⧉</button></div><div className="password-row"><label htmlFor="device-password">SESSION PASSWORD</label><div className="password-control"><input id="device-password" className="mono password-value" type="password" value={identity?.password ?? ""} readOnly aria-label="Session password" /><button className="quiet-button" type="button" onClick={() => void rotate()}>Rotate</button></div></div><p className="caption">Share your ID and password to let someone control this device.</p></section>
      <section className="card connect-card"><span className="eyebrow">CONTROL A REMOTE DEVICE</span><form onSubmit={(event) => void submit(event)}><div className="connect-fields"><input className="mono target-input" inputMode="numeric" maxLength={9} placeholder="000 000 000" value={targetId} onChange={(event) => setTargetId(event.target.value.replace(/\D/g, ""))} aria-label="Remote device ID" /><input className="mono remote-password-input" type="password" placeholder="Password" value={remotePassword} onChange={(event) => setRemotePassword(event.target.value)} aria-label="Remote device password" /><button className="primary-button" type="submit" disabled={busy}>{busy ? "Connecting…" : "Connect"}</button></div></form>{recentIds.length > 0 && <div className="recent"><span className="recent-label">RECENT</span>{recentIds.map((id) => <button className="chip mono" type="button" key={id} onClick={() => setTargetId(id)}>{id}</button>)}</div>}</section>
    </div>
    {message && <p className="message" role="status">{message}</p>}
    {incomingRequest && <div className="request-notice" role="dialog"><strong>Incoming request</strong><span className="mono">{incomingRequest.peer_id}</span><button type="button" onClick={onDismissRequest}>Review in next task</button></div>}
  </main>;
}
