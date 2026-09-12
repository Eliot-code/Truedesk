import { useState } from "react";
import * as api from "../lib/api";

export interface IncomingRequestProps {
  peerId: string;
  fingerprint: string;
  onResolved: () => void;
}

export default function IncomingRequest({ peerId, fingerprint, onResolved }: IncomingRequestProps) {
  const [alwaysChecked, setAlwaysChecked] = useState(false);
  const [busy, setBusy] = useState(false);

  const resolve = async (allow: boolean) => {
    if (busy) return;
    setBusy(true);
    try {
      await api.respondToRequest(peerId, allow, allow && alwaysChecked);
      onResolved();
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="incoming-backdrop" role="presentation">
      <section className="incoming-modal" role="dialog" aria-modal="true" aria-labelledby="incoming-request-title">
        <div className="incoming-icon" aria-hidden="true">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7">
            <path d="M12 3 20 6v5c0 5-3.4 8.4-8 10-4.6-1.6-8-5-8-10V6l8-3Z" />
            <path d="m8.7 12 2.2 2.2 4.5-4.6" />
          </svg>
        </div>
        <span className="eyebrow">INCOMING REQUEST</span>
        <h1 id="incoming-request-title">Allow remote control?</h1>
        <p className="incoming-lead">A device wants to view and control this computer.</p>

        <div className="request-details">
          <div><span className="request-label">REQUESTING DEVICE</span><strong className="mono request-peer">{peerId}</strong></div>
          <div className="request-fingerprint"><span className="request-label">FINGERPRINT</span><strong className="mono">{fingerprint}</strong><small>Verify this matches the other device.</small></div>
        </div>

        <div className="permission-summary" aria-label="Requested permissions">
          <div><span className="permission-check" aria-hidden="true">✓</span><span>See your screen</span></div>
          <div><span className="permission-check" aria-hidden="true">✓</span><span>Control mouse &amp; keyboard</span></div>
        </div>

        <label className="always-allow"><input type="checkbox" checked={alwaysChecked} onChange={(event) => setAlwaysChecked(event.target.checked)} /> <span>Always allow this device</span></label>
        <div className="incoming-actions">
          <button className="ghost-button" type="button" disabled={busy} onClick={() => void resolve(false)}>Reject</button>
          <button className="primary-button incoming-allow" type="button" disabled={busy} onClick={() => void resolve(true)}>Allow</button>
        </div>
        <p className="incoming-footer">End-to-end encrypted — OpenDesk never sees your screen.</p>
      </section>
    </div>
  );
}
