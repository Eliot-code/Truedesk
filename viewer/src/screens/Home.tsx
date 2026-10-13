import { useEffect, useRef, useState, type FormEvent } from "react";
import * as api from "../lib/api";
import { friendlyError, useNotify } from "../lib/notify";
import QrCode from "../components/QrCode";

interface HomeProps {
  status: api.Status;
  /** Kept optional for callers from the pre-integration shell. */
  incomingRequest?: api.IncomingRequest | null;
  onDismissRequest?: () => void;
  onOpenPermissions?: () => void;
  /** Pre-fills the connect form, e.g. after a rejected attempt. */
  initialTargetId?: string;
  onConnect: (targetId: string, password: string, authCode?: string) => Promise<void>;
}
const statusLabels: Record<api.Status, string> = { offline: "Offline", online: "Online", connecting: "Connecting", in_session: "In session" };

/** Base32 secret in groups of four for manual entry. */
const groupSecret = (secret: string) => secret.replace(/(.{4})(?=.)/g, "$1 ");

export default function Home({ status, onConnect, onOpenPermissions, initialTargetId = "" }: HomeProps) {
  const notify = useNotify();
  const setMessage = (text: string | null, tone: "error" | "info" = "error") => { if (text) notify(text, tone); };
  const passwordInputRef = useRef<HTMLInputElement>(null);
  const [identity, setIdentity] = useState<api.Identity | null>(null);
  const [showPassword, setShowPassword] = useState(false);
  const [recentIds, setRecentIds] = useState<string[]>([]);
  const [contacts, setContacts] = useState<api.Contact[]>([]);
  const [allowedDevices, setAllowedDevices] = useState<string[]>([]);
  const [allowedId, setAllowedId] = useState("");
  const [targetId, setTargetId] = useState(initialTargetId);
  const [remotePassword, setRemotePassword] = useState("");
  const [authCode, setAuthCode] = useState("");
  const [alias, setAlias] = useState("");
  const [contactId, setContactId] = useState("");
  const [busy, setBusy] = useState(false);
  const [unattended, setUnattended] = useState<api.UnattendedStatus>({ enabled: false, has_password: false, twofa_enabled: false, ready: false, problem: null });
  const [unattendedEnabled, setUnattendedEnabled] = useState(false);
  const [unattendedPassword, setUnattendedPassword] = useState("");
  const [enrollment, setEnrollment] = useState<api.TotpEnrollment | null>(null);
  const [enrollmentCode, setEnrollmentCode] = useState("");
  const [twofaBusy, setTwofaBusy] = useState(false);
  // null until the OS answers (or if it can't).
  const [startAtSignIn, setStartAtSignIn] = useState<boolean | null>(null);
  const [startAtSignInBusy, setStartAtSignInBusy] = useState(false);
  const [relays, setRelays] = useState<string[]>([]);
  const [closeKeepsRunning, setCloseKeepsRunning] = useState(false);
  const [relayText, setRelayText] = useState("");

  useEffect(() => {
    let mounted = true;
    void Promise.all([api.getIdentity(), api.getConfig(), api.getUnattendedStatus(), api.listContacts(), api.listAllowed()]).then(([loadedIdentity, config, unattendedStatus, loadedContacts, loadedAllowed]) => {
      if (mounted) { const configuredRelays = config.relay_urls?.length ? config.relay_urls : [config.relay_url]; setIdentity(loadedIdentity); setRecentIds(config.recent_ids); setRelays(configuredRelays); setRelayText(configuredRelays.join("\n")); setUnattended(unattendedStatus); setUnattendedEnabled(unattendedStatus.enabled); setContacts(loadedContacts); setAllowedDevices(loadedAllowed); setMessage(unattendedStatus.problem); }
    }).catch((error: unknown) => mounted && setMessage(friendlyError(error)));
    void api.getStartAtSignIn().then((enabled) => { if (mounted) setStartAtSignIn(enabled); }).catch(() => undefined);
    void api.startHosting().catch((error: unknown) => mounted && setMessage(friendlyError(error)));
    // The backend rotates the session password after each session and after
    // repeated failed logins.
    const passwordListener = api.onPasswordRotated(({ password }) => { if (mounted) setIdentity((current) => current ? { ...current, password } : current); }).catch(() => undefined);
    // A new device's random ID can collide with another device's; the backend picks a new one.
    const idListener = api.onIdentityChanged(({ id }) => { if (mounted) setIdentity((current) => current ? { ...current, id } : current); }).catch(() => undefined);
    return () => { mounted = false; void passwordListener.then((unlisten) => unlisten?.()); void idListener.then((unlisten) => unlisten?.()); };
  }, []);

  // Closing the window may only hide it, so offer a way to quit from here.
  useEffect(() => {
    let mounted = true;
    void api.closeKeepsRunning().then((keeps) => { if (mounted) setCloseKeepsRunning(keeps); }).catch(() => undefined);
    return () => { mounted = false; };
  }, [startAtSignIn, unattended.enabled, unattended.has_password]);

  const rotate = async () => {
    try { const result = await api.rotatePassword(); setIdentity((current) => current ? { ...current, password: result.password } : current); setMessage("New password generated.", "info"); }
    catch (error) { setMessage(friendlyError(error)); }
  };
  const saveUnattended = async () => {
    try {
      const saved = await api.setUnattendedAccess(unattendedEnabled, unattendedPassword || null);
      setUnattended(saved);
      setUnattendedPassword("");
      setMessage(unattendedEnabled ? "Unattended access is on." : "Unattended access is off.", "info");
    } catch (error) {
      setUnattendedEnabled(unattended.enabled);
      setMessage(friendlyError(error));
    }
  };
  const saveRelays = async () => {
    const urls = relayText.split(/\r?\n|,/).map((url) => url.trim()).filter(Boolean);
    if (!urls.length) { setMessage("Enter at least one relay URL."); return; }
    try { await api.setRelays(urls); setRelays(urls); setRelayText(urls.join("\n")); setMessage("Relay regions updated.", "info"); } catch (error) { setMessage(friendlyError(error)); }
  };
  // Turning 2FA on only starts an enrollment; it becomes active once a code
  // from the authenticator is confirmed.
  const toggleTwofa = async (enabled: boolean) => {
    if (twofaBusy) return;
    setTwofaBusy(true);
    try {
      if (enabled) {
        setEnrollment(await api.setUnattended2fa(true));
        setEnrollmentCode("");
      } else {
        await api.setUnattended2fa(false);
        setEnrollment(null);
        setUnattended((current) => ({ ...current, twofa_enabled: false }));
        // Turning off 2FA can clear a "2FA secret is missing" problem.
        void api.getUnattendedStatus().then(setUnattended).catch(() => undefined);
        setMessage("Two-factor is off.", "info");
      }
    } catch (error) { setMessage(friendlyError(error)); } finally { setTwofaBusy(false); }
  };
  const confirmTwofa = async (event: FormEvent) => {
    event.preventDefault();
    if (!/^\d{6}$/.test(enrollmentCode)) { setMessage("Enter the 6-digit code from your authenticator app."); return; }
    setTwofaBusy(true);
    try {
      const saved = await api.confirmUnattended2fa(enrollmentCode);
      setEnrollment(null);
      setEnrollmentCode("");
      setUnattended(saved);
      setMessage("Two-factor is on.", "info");
    } catch (error) { setMessage(friendlyError(error)); } finally { setTwofaBusy(false); }
  };
  const cancelTwofa = async () => {
    setTwofaBusy(true);
    try { await api.setUnattended2fa(false); setEnrollment(null); setEnrollmentCode(""); }
    catch (error) { setMessage(friendlyError(error)); } finally { setTwofaBusy(false); }
  };
  const toggleStartAtSignIn = async (enabled: boolean) => {
    setStartAtSignInBusy(true);
    try { await api.setStartAtSignIn(enabled); setStartAtSignIn(enabled); }
    catch (error) { setMessage(friendlyError(error)); } finally { setStartAtSignInBusy(false); }
  };
  const connect = async (id: string) => {
    if (!remotePassword) { setMessage("Enter the remote device password."); return; }
    setBusy(true);
    try { const code = authCode.trim(); await (code ? onConnect(id, remotePassword, code) : onConnect(id, remotePassword)); } catch (error) { setMessage(friendlyError(error)); } finally { setAuthCode(""); setBusy(false); }
  };
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!/^\d{9}$/.test(targetId)) { setMessage("Enter the 9-digit ID."); return; }
    await connect(targetId);
  };
  // Fill the form rather than reusing whatever password happens to be typed:
  // each device has its own password.
  const connectContact = (contact: api.Contact) => {
    setTargetId(contact.id);
    setRemotePassword("");
    setAuthCode("");
    passwordInputRef.current?.focus();
  };
  const add = async (event: FormEvent) => {
    event.preventDefault();
    if (!alias.trim()) { setMessage("Enter a name for the device."); return; }
    if (!/^\d{9}$/.test(contactId)) { setMessage("Enter the 9-digit ID."); return; }
    try { const contact = await api.addContact(alias.trim(), contactId); setContacts((current) => { const without = current.filter((item) => item.id !== contact.id); return [...without, contact]; }); setAlias(""); setContactId(""); } catch (error) { setMessage(friendlyError(error)); }
  };
  const remove = async (id: string) => {
    try { await api.removeContact(id); setContacts((current) => current.filter((contact) => contact.id !== id)); } catch (error) { setMessage(friendlyError(error)); }
  };
  const addAllowedDevice = async (event: FormEvent) => {
    event.preventDefault();
    if (!/^\d{9}$/.test(allowedId)) { setMessage("Enter the 9-digit ID."); return; }
    try { await api.addAllowed(allowedId); setAllowedDevices((current) => current.includes(allowedId) ? current : [...current, allowedId]); setAllowedId(""); } catch (error) { setMessage(friendlyError(error)); }
  };
  const removeAllowedDevice = async (id: string) => {
    try { await api.removeAllowed(id); setAllowedDevices((current) => current.filter((device) => device !== id)); } catch (error) { setMessage(friendlyError(error)); }
  };
  const canEnableUnattended = unattended.has_password || unattendedPassword.length > 0;
  const unattendedDirty = unattendedEnabled !== unattended.enabled || unattendedPassword.length > 0;
  // 2FA protects unattended access, so it can only be turned on once that is
  // saved as enabled; turning it off is always allowed.
  const twofaToggleEnabled = unattended.twofa_enabled || unattended.enabled;
  // Starting at sign-in is only useful when someone can connect without
  // approval, so it waits for saved unattended access whose password the
  // keyring can actually return. Turning it off is always allowed.
  const unattendedReady = unattended.ready;
  const startAtSignInHint = startAtSignIn === null ? null
    : !unattendedReady && unattended.enabled ? (startAtSignIn ? "Unattended access isn't working, so OpenDesk will just open its window at sign-in." : "Fix unattended access first.")
    : !unattendedReady ? (startAtSignIn ? "Unattended access is off, so OpenDesk will just open its window at sign-in." : "Turn on and save unattended access first.")
    : startAtSignIn ? "OpenDesk starts hidden and keeps hosting. Closing the window leaves it running; use Quit OpenDesk to stop it." : null;
  const relativeLastConnected = (timestamp: number | null) => {
    if (timestamp === null) return "Never connected";
    const seconds = Math.max(0, Math.floor(Date.now() / 1000) - timestamp);
    if (seconds < 60) return "Just now";
    if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
    if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
    return `${Math.floor(seconds / 86400)}d ago`;
  };
  const startHosting = () => { void api.startHosting().catch((error: unknown) => setMessage(friendlyError(error))); };
  const copyId = async () => {
    if (!identity) return;
    try { await navigator.clipboard.writeText(identity.id); setMessage("ID copied.", "info"); } catch { setMessage("Could not copy ID."); }
  };
  const copyPassword = async () => {
    if (!identity) return;
    try { await navigator.clipboard.writeText(identity.password); setMessage("Password copied.", "info"); } catch { setMessage("Could not copy password."); }
  };

  return <main className="app-shell">
    <header className="topbar"><div className="brand"><span className="lock-glyph" aria-hidden="true">⌑</span> OpenDesk</div><div className="topbar-actions"><button className="quiet-button" type="button" onClick={onOpenPermissions}>Permissions</button><div className="status" data-status={status}><span className="status-dot" aria-hidden="true" /><span>{statusLabels[status]}</span></div>{status === "offline" && <button className="quiet-button" type="button" onClick={startHosting}>Start hosting</button>}{closeKeepsRunning && <button className="quiet-button" type="button" onClick={() => void api.quitApp().catch(() => undefined)}>Quit OpenDesk</button>}</div></header>
    <section className="hero"><span className="eyebrow">PERSONAL DEVICE ACCESS</span><h1>Remote access, end to end.</h1><p>Share your ID to let someone help you, or enter another device's ID to control it.</p></section>
    <div className="cards">
      <section className="card identity-card"><span className="eyebrow">YOUR ID</span><div className="id-row"><strong className="mono identity-value">{identity?.id ?? "—"}</strong><button className="icon-button" type="button" onClick={() => void copyId()} aria-label="Copy device ID">⧉</button></div><div className="password-row"><label htmlFor="device-password">SESSION PASSWORD</label><div className="password-control"><input id="device-password" className="mono password-value" type={showPassword ? "text" : "password"} value={identity?.password ?? ""} readOnly aria-label="Session password" /><button className="quiet-button" type="button" onClick={() => setShowPassword((shown) => !shown)} aria-label={showPassword ? "Hide password" : "Show password"}>{showPassword ? "Hide" : "Show"}</button><button className="icon-button" type="button" onClick={() => void copyPassword()} aria-label="Copy password">⧉</button><button className="quiet-button" type="button" onClick={() => void rotate()}>Rotate</button></div></div><p className="caption">Share your ID and password to let someone control this device.</p></section>
      <section className="card connect-card"><span className="eyebrow">CONTROL A REMOTE DEVICE</span><form onSubmit={(event) => void submit(event)}><div className="connect-fields"><input className="mono target-input" inputMode="numeric" maxLength={9} placeholder="000 000 000" value={targetId} onChange={(event) => setTargetId(event.target.value.replace(/\D/g, ""))} aria-label="Remote device ID" /><input ref={passwordInputRef} className="mono remote-password-input" type="password" placeholder="Password" value={remotePassword} onChange={(event) => setRemotePassword(event.target.value)} aria-label="Remote device password" /><input className="mono remote-password-input" type="text" inputMode="numeric" maxLength={6} pattern="[0-9]{6}" autoComplete="one-time-code" placeholder="2FA code (optional)" aria-label="2FA code (optional)" value={authCode} onChange={(event) => setAuthCode(event.target.value.replace(/\D/g, ""))} /><button className="primary-button" type="submit" disabled={busy}>{busy ? "Connecting…" : "Connect"}</button></div></form>{recentIds.length > 0 && <div className="recent"><span className="recent-label">RECENT</span>{recentIds.map((id) => <button className="chip mono" type="button" key={id} onClick={() => setTargetId(id)}>{id}</button>)}</div>}</section>
      <section className="card unattended-card"><div className="unattended-heading"><div><span className="eyebrow">UNATTENDED ACCESS</span><h2>Connect without approval</h2></div><label className="toggle-label" title={canEnableUnattended ? undefined : "Set a password first"}><input type="checkbox" checked={unattendedEnabled} disabled={!canEnableUnattended} onChange={(event) => setUnattendedEnabled(event.target.checked)} /><span>Enabled</span></label></div><input className="password-value" type="password" placeholder={unattended.has_password ? "Enter a new password to replace it" : "Set a password (12+ characters)"} value={unattendedPassword} onChange={(event) => setUnattendedPassword(event.target.value)} aria-label="Unattended access password" /><button className="quiet-button unattended-save" type="button" disabled={!unattendedDirty} onClick={() => void saveUnattended()}>Save settings</button>{unattendedDirty && <p className="caption unattended-pending">Changes apply after you save.</p>}{unattended.problem && <p className="unattended-problem" role="alert">{unattended.problem}</p>}<label className="toggle-label" title={twofaToggleEnabled ? undefined : "Turn on and save unattended access first"}><input type="checkbox" checked={unattended.twofa_enabled} disabled={!twofaToggleEnabled || twofaBusy || enrollment !== null} onChange={(event) => void toggleTwofa(event.target.checked)} /><span>Require 2FA code</span></label>{enrollment && <form className="twofa-enroll" onSubmit={(event) => void confirmTwofa(event)} aria-label="Set up two-factor"><QrCode width={enrollment.qr_width} modules={enrollment.qr_modules} label="QR code for your authenticator app" /><div className="twofa-enroll__details"><p className="caption">Scan with an authenticator app, or enter this key manually:</p><code className="mono twofa-secret" aria-label="Setup key">{groupSecret(enrollment.secret)}</code><input className="password-value mono" inputMode="numeric" autoComplete="one-time-code" maxLength={6} placeholder="6-digit code" aria-label="Authenticator code" value={enrollmentCode} onChange={(event) => setEnrollmentCode(event.target.value.replace(/\D/g, ""))} /><div className="twofa-enroll__actions"><button className="ghost-button" type="button" disabled={twofaBusy} onClick={() => void cancelTwofa()}>Cancel</button><button className="primary-button" type="submit" disabled={twofaBusy}>Confirm</button></div></div></form>}<label className="toggle-label"><input type="checkbox" checked={startAtSignIn ?? false} disabled={startAtSignIn === null || startAtSignInBusy || (!startAtSignIn && !unattendedReady)} onChange={(event) => void toggleStartAtSignIn(event.target.checked)} /><span>Start OpenDesk when I sign in</span></label>{startAtSignInHint && <p className="caption start-at-sign-in-hint">{startAtSignInHint}</p>}<p className="caption">Anyone with this password can connect without approval. Protect it carefully.</p></section>
      <section className="card relay-card"><span className="eyebrow">RELAY REGIONS</span><h2>Relay endpoints</h2><textarea className="relay-editor mono" value={relayText} onChange={(event) => setRelayText(event.target.value)} aria-label="Relay URLs" rows={Math.max(2, relays.length)} /><button className="quiet-button" type="button" onClick={() => void saveRelays()}>Save relay list</button><p className="caption">One WebSocket URL per line. OpenDesk tries them in this order and uses the first reachable relay.</p></section>
      <section className="card address-book"><div className="address-book-heading"><span className="eyebrow">ADDRESS BOOK</span><span className="caption">Saved devices</span></div>{contacts.length > 0 ? <div className="contact-list">{contacts.map((contact) => <div className="contact-row" key={contact.id}><div><strong>{contact.alias}</strong><span className="mono">{contact.id}</span><small>{relativeLastConnected(contact.last_connected)}</small></div><div className="contact-actions"><button className="quiet-button" type="button" disabled={busy} onClick={() => connectContact(contact)}>Connect</button><button className="icon-button" type="button" onClick={() => void remove(contact.id)} aria-label={`Remove ${contact.alias}`}>×</button></div></div>)}</div> : <p className="caption">Save a device to reconnect without retyping its ID.</p>}<form className="contact-form" onSubmit={(event) => void add(event)}><input value={alias} onChange={(event) => setAlias(event.target.value)} placeholder="Alias" aria-label="Contact alias" /><input className="mono" inputMode="numeric" maxLength={9} value={contactId} onChange={(event) => setContactId(event.target.value.replace(/\D/g, ""))} placeholder="Device ID" aria-label="Contact device ID" /><button className="primary-button" type="submit">Add</button></form></section>
      <section className="card allow-list-card"><span className="eyebrow">ALLOWED DEVICES</span><h2>Known devices</h2><p className="caption">Known devices are marked in the approval prompt. They still need your approval and the session password.</p>{allowedDevices.length > 0 ? <div className="contact-list">{allowedDevices.map((id) => <div className="contact-row" key={id}><span className="mono">{id}</span><button className="icon-button" type="button" onClick={() => void removeAllowedDevice(id)} aria-label={`Remove allowed device ${id}`}>×</button></div>)}</div> : <p className="caption">No devices are allowed yet.</p>}<form className="contact-form" onSubmit={(event) => void addAllowedDevice(event)}><input className="mono" inputMode="numeric" maxLength={9} value={allowedId} onChange={(event) => setAllowedId(event.target.value.replace(/\D/g, ""))} placeholder="Device ID" aria-label="Allowed device ID" /><button className="primary-button" type="submit">Add</button></form></section>
    </div>
  </main>;
}
