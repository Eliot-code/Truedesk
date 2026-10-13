import { useState, type FormEvent } from "react";
import { formatId, normalizeId, relayUrlError } from "../lib/settings";

export interface ConnectRequest {
  target: string;
  password: string;
  authCode: string;
  relay: string;
}

export interface ConnectFormProps {
  viewerId: string;
  relay: string;
  defaultRelay: string;
  /** Shown above the form, e.g. why the last session ended. */
  message?: string;
  /** Set when the browser can't run the viewer at all. */
  unsupported?: string | null;
  busy?: boolean;
  initialTarget?: string;
  onConnect(request: ConnectRequest): void;
}

export default function ConnectForm({ viewerId, relay: savedRelay, defaultRelay, message, unsupported, busy = false, initialTarget = "", onConnect }: ConnectFormProps) {
  const [target, setTarget] = useState(initialTarget);
  const [password, setPassword] = useState("");
  const [authCode, setAuthCode] = useState("");
  const [relay, setRelay] = useState(savedRelay);
  const [problem, setProblem] = useState<string | null>(null);

  const submit = (event: FormEvent) => {
    event.preventDefault();
    const id = normalizeId(target);
    if (!/^\d{9}$/.test(id)) return setProblem("Enter the 9-digit ID.");
    if (!password) return setProblem("Enter the password shown on the other device.");
    const code = authCode.trim();
    if (code && !/^\d{6}$/.test(code)) return setProblem("The two-factor code has 6 digits.");
    const relayUrl = relay.trim() || defaultRelay;
    const relayProblem = relayUrlError(relayUrl);
    if (relayProblem) return setProblem(relayProblem);
    setProblem(null);
    onConnect({ target: id, password, authCode: code, relay: relayUrl });
  };

  const notice = problem ?? message;

  return (
    <main className="app-shell web-shell">
      <header className="topbar">
        <a className="brand" href="https://opendesk.treeot.dev">
          <svg className="brand-mark" viewBox="0 0 48 48" aria-hidden="true"><g fill="none" strokeWidth="6"><circle cx="19" cy="29" r="13" stroke="#22d3ee" /><circle cx="29" cy="19" r="13" stroke="#f5b921" /></g><circle cx="29" cy="19" r="5" fill="#22d3ee" /></svg>
          OpenDesk
        </a>
        <div className="status mono" title="The host sees this ID when you connect">
          YOUR ID <span className="web-own-id">{formatId(viewerId)}</span>
        </div>
      </header>
      <section className="hero web-hero">
        <span className="eyebrow">BROWSER VIEWER</span>
        <h1>Control a computer from here.</h1>
        <p>Enter the ID and password shown in OpenDesk on the other computer. Nothing to install. If that computer has unattended access turned on, use its unattended password, not the one on screen.</p>
      </section>
      <section className="card connect-card web-connect-card">
        {unsupported && <p className="message" role="alert">{unsupported}</p>}
        {!unsupported && notice && <p className="message" role="alert">{notice}</p>}
        <form onSubmit={submit} noValidate>
          <div className="connect-fields">
            <input className="target-input mono" aria-label="Device ID" placeholder="Device ID (123 456 789)" inputMode="numeric" autoComplete="off" value={target} onChange={(event) => setTarget(event.target.value)} />
            <input className="remote-password-input" aria-label="Password" placeholder="Password" type="password" autoComplete="off" value={password} onChange={(event) => setPassword(event.target.value)} />
            <input className="remote-password-input mono" aria-label="Two-factor code" placeholder="2FA code (if required)" inputMode="numeric" autoComplete="one-time-code" maxLength={6} value={authCode} onChange={(event) => setAuthCode(event.target.value)} />
            <button className="primary-button" type="submit" disabled={busy || Boolean(unsupported)}>{busy ? "Connecting…" : "Connect"}</button>
          </div>
          <details className="web-advanced">
            <summary>Relay</summary>
            <input className="remote-password-input mono" aria-label="Relay URL" spellCheck={false} value={relay} onChange={(event) => setRelay(event.target.value)} />
            <p className="caption">Use the same relay as the other computer. Default: <span className="mono">{defaultRelay}</span></p>
          </details>
        </form>
        <p className="caption web-reassure">
          Everything you see and do stays encrypted all the way to the other computer. The relay only passes sealed data through &mdash; it can't read your screen or what you type.
          For sound, file transfer and clipboard sharing, use the desktop app.
        </p>
      </section>
    </main>
  );
}
