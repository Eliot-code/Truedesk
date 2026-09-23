import { useEffect, useState, type FormEvent } from "react";
import * as api from "../lib/api";

interface HomeProps {
  status: api.Status;
  /** Kept optional for callers from the pre-integration shell. */
  incomingRequest?: api.IncomingRequest | null;
  onDismissRequest?: () => void;
  onOpenPermissions?: () => void;
  onConnect: (targetId: string, password: string) => Promise<void>;
}
const statusLabels: Record<api.Status, string> = { offline: "Offline", online: "Online", connecting: "Connecting", in_session: "In session" };

export default function Home({ status, onConnect, onOpenPermissions }: HomeProps) {
  const [identity, setIdentity] = useState<api.Identity | null>(null);
  const [recentIds, setRecentIds] = useState<string[]>([]);
  const [contacts, setContacts] = useState<api.Contact[]>([]);
  const [targetId, setTargetId] = useState("");
  const [remotePassword, setRemotePassword] = useState("");
  const [alias, setAlias] = useState("");
  const [contactId, setContactId] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [unattended, setUnattended] = useState<api.UnattendedStatus>({ enabled: false, has_password: false });
  const [unattendedPassword, setUnattendedPassword] = useState("");
  const [relays, setRelays] = useState<string[]>([]);
  const [relayText, setRelayText] = useState("");

  useEffect(() => {
    let mounted = true;
    void Promise.all([api.getIdentity(), api.getConfig(), api.getUnattendedStatus(), api.listContacts()]).then(([loadedIdentity, config, unattendedStatus, loadedContacts]) => {
      if (mounted) { const configuredRelays = config.relay_urls?.length ? config.relay_urls : [config.relay_url]; setIdentity(loadedIdentity); setRecentIds(config.recent_ids); setRelays(configuredRelays); setRelayText(configuredRelays.join("\n")); setUnattended(unattendedStatus); setContacts(loadedContacts); }
    }).catch((error: unknown) => mounted && setMessage(String(error)));
    void api.startHosting().catch((error: unknown) => mounted && setMessage(String(error)));
    return () => { mounted = false; };
  }, []);

  const rotate = async () => {
    try { const result = await api.rotatePassword(); setIdentity((current) => current ? { ...current, password: result.password } : current); }
    catch (error) { setMessage(String(error)); }
  };
  const saveUnattended = async () => {
    try {
      await api.setUnattendedAccess(unattended.enabled, unattendedPassword || null);
      setUnattended({ enabled: unattended.enabled, has_password: Boolean(unattendedPassword) });
      setUnattendedPassword("");
      setMessage("Unattended access updated.");
    } catch (error) { setMessage(String(error)); }
  };
  const saveRelays = async () => {
    const urls = relayText.split(/\r?\n|,/).map((url) => url.trim()).filter(Boolean);
    if (!urls.length) { setMessage("Enter at least one relay URL."); return; }
    try { await api.setRelays(urls); setRelays(urls); setRelayText(urls.join("\n")); setMessage("Relay regions updated."); } catch (error) { setMessage(String(error)); }
  };
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!/^\d{9}$/.test(targetId)) { setMessage("Enter a 9-digit device ID."); return; }
    if (!remotePassword) { setMessage("Enter the remote device password."); return; }
    setBusy(true); setMessage(null);
    try { await onConnect(targetId, remotePassword); } catch (error) { setMessage(String(error)); } finally { setBusy(false); }
  };
  const connectContact = async (contact: api.Contact) => {
    if (!remotePassword) { setMessage("Enter the remote device password."); return; }
    setBusy(true); setMessage(null);
    try { await onConnect(contact.id, remotePassword); } catch (error) { setMessage(String(error)); } finally { setBusy(false); }
  };
  const add = async (event: FormEvent) => {
    event.preventDefault();
    if (!alias.trim() || !contactId.trim()) { setMessage("Enter an alias and device ID."); return; }
    try { const contact = await api.addContact(alias, contactId); setContacts((current) => { const without = current.filter((item) => item.id !== contact.id); return [...without, contact]; }); setAlias(""); setContactId(""); setMessage(null); } catch (error) { setMessage(String(error)); }
  };
  const remove = async (id: string) => {
    try { await api.removeContact(id); setContacts((current) => current.filter((contact) => contact.id !== id)); } catch (error) { setMessage(String(error)); }
  };
  const relativeLastConnected = (timestamp: number | null) => {
    if (timestamp === null) return "Never connected";
    const seconds = Math.max(0, Math.floor(Date.now() / 1000) - timestamp);
    if (seconds < 60) return "Just now";
    if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
    if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
    return `${Math.floor(seconds / 86400)}d ago`;
  };
  const copyId = async () => {
    if (!identity) return;
    try { await navigator.clipboard.writeText(identity.id); setMessage("ID copied."); } catch { setMessage("Could not copy ID."); }
  };

  return <main className="app-shell">
    <header className="topbar"><div className="brand"><span className="lock-glyph" aria-hidden="true">⌑</span> OpenDesk</div><div className="topbar-actions"><button className="quiet-button" type="button" onClick={onOpenPermissions}>Permissions</button><div className="status" data-status={status}><span className="status-dot" aria-hidden="true" /><span>{statusLabels[status]}</span></div></div></header>
    <section className="hero"><span className="eyebrow">PERSONAL DEVICE ACCESS</span><h1>Welcome back.</h1><p>Your device is ready when you are.</p></section>
    <div className="cards">
      <section className="card identity-card"><span className="eyebrow">YOUR ID</span><div className="id-row"><strong className="mono identity-value">{identity?.id ?? "—"}</strong><button className="icon-button" type="button" onClick={() => void copyId()} aria-label="Copy device ID">⧉</button></div><div className="password-row"><label htmlFor="device-password">SESSION PASSWORD</label><div className="password-control"><input id="device-password" className="mono password-value" type="password" value={identity?.password ?? ""} readOnly aria-label="Session password" /><button className="quiet-button" type="button" onClick={() => void rotate()}>Rotate</button></div></div><p className="caption">Share your ID and password to let someone control this device.</p></section>
      <section className="card connect-card"><span className="eyebrow">CONTROL A REMOTE DEVICE</span><form onSubmit={(event) => void submit(event)}><div className="connect-fields"><input className="mono target-input" inputMode="numeric" maxLength={9} placeholder="000 000 000" value={targetId} onChange={(event) => setTargetId(event.target.value.replace(/\D/g, ""))} aria-label="Remote device ID" /><input className="mono remote-password-input" type="password" placeholder="Password" value={remotePassword} onChange={(event) => setRemotePassword(event.target.value)} aria-label="Remote device password" /><button className="primary-button" type="submit" disabled={busy}>{busy ? "Connecting…" : "Connect"}</button></div></form>{recentIds.length > 0 && <div className="recent"><span className="recent-label">RECENT</span>{recentIds.map((id) => <button className="chip mono" type="button" key={id} onClick={() => setTargetId(id)}>{id}</button>)}</div>}</section>
      <section className="card unattended-card"><div className="unattended-heading"><div><span className="eyebrow">UNATTENDED ACCESS</span><h2>Connect without approval</h2></div><label className="toggle-label"><input type="checkbox" checked={unattended.enabled} onChange={(event) => setUnattended((current) => ({ ...current, enabled: event.target.checked }))} /><span>Enabled</span></label></div><input className="password-value" type="password" placeholder={unattended.has_password ? "Enter a new password to replace it" : "Set a password"} value={unattendedPassword} onChange={(event) => setUnattendedPassword(event.target.value)} aria-label="Unattended access password" /><button className="quiet-button unattended-save" type="button" onClick={() => void saveUnattended()}>Save settings</button><p className="caption">Anyone with this password can connect without approval. Protect it carefully.</p></section>
      <section className="card relay-card"><span className="eyebrow">RELAY REGIONS</span><h2>Relay endpoints</h2><textarea className="relay-editor mono" value={relayText} onChange={(event) => setRelayText(event.target.value)} aria-label="Relay URLs" rows={Math.max(2, relays.length)} /><button className="quiet-button" type="button" onClick={() => void saveRelays()}>Save relay list</button><p className="caption">One WebSocket URL per line. OpenDesk tries them in this order and uses the first reachable relay.</p></section>
      <section className="card address-book"><div className="address-book-heading"><span className="eyebrow">ADDRESS BOOK</span><span className="caption">Known devices</span></div>{contacts.length > 0 ? <div className="contact-list">{contacts.map((contact) => <div className="contact-row" key={contact.id}><div><strong>{contact.alias}</strong><span className="mono">{contact.id}</span><small>{relativeLastConnected(contact.last_connected)}</small></div><div className="contact-actions"><button className="quiet-button" type="button" disabled={busy} onClick={() => void connectContact(contact)}>Connect</button><button className="icon-button" type="button" onClick={() => void remove(contact.id)} aria-label={`Remove ${contact.alias}`}>×</button></div></div>)}</div> : <p className="caption">Save a device to reconnect without retyping its ID.</p>}<form className="contact-form" onSubmit={(event) => void add(event)}><input value={alias} onChange={(event) => setAlias(event.target.value)} placeholder="Alias" aria-label="Contact alias" /><input className="mono" inputMode="numeric" maxLength={9} value={contactId} onChange={(event) => setContactId(event.target.value.replace(/\D/g, ""))} placeholder="Device ID" aria-label="Contact device ID" /><button className="primary-button" type="submit">Add</button></form></section>
    </div>
    {message && <p className="message" role="status">{message}</p>}
  </main>;
}
