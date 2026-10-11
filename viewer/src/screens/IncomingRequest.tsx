import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import * as api from "../lib/api";
import { friendlyError, useNotify } from "../lib/notify";

export interface IncomingRequestProps {
  peerId: string;
  /** On the known-devices list; still needs approval. */
  known?: boolean;
  onResolved: () => void;
}

const FOCUSABLE = "button:not([disabled]), input:not([disabled]), [href], [tabindex]:not([tabindex='-1'])";

export default function IncomingRequest({ peerId, known = false, onResolved }: IncomingRequestProps) {
  const [alwaysChecked, setAlwaysChecked] = useState(false);
  const [busy, setBusy] = useState(false);
  const dialogRef = useRef<HTMLElement>(null);
  const rejectRef = useRef<HTMLButtonElement>(null);
  const notify = useNotify();

  // Reject is the safe default: an accidental Enter must not grant control.
  useEffect(() => { rejectRef.current?.focus(); }, []);

  const resolve = async (allow: boolean) => {
    if (busy) return;
    setBusy(true);
    try {
      await api.respondToRequest(peerId, allow, allow && alwaysChecked);
      onResolved();
    } catch (error) {
      notify(friendlyError(error));
    } finally {
      setBusy(false);
    }
  };

  const onKeyDown = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key === "Escape") {
      event.preventDefault();
      void resolve(false);
      return;
    }
    if (event.key !== "Tab" || !dialogRef.current) return;
    const focusable = Array.from(dialogRef.current.querySelectorAll<HTMLElement>(FOCUSABLE));
    if (focusable.length === 0) return;
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    const current = document.activeElement;
    if (event.shiftKey && (current === first || !dialogRef.current.contains(current))) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && (current === last || !dialogRef.current.contains(current))) {
      event.preventDefault();
      first.focus();
    }
  };

  return (
    <div className="incoming-backdrop" role="presentation">
      <section ref={dialogRef} className="incoming-modal" role="dialog" aria-modal="true" aria-labelledby="incoming-request-title" onKeyDown={onKeyDown}>
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
          <div><span className="request-label">REQUESTING DEVICE</span><strong className="mono request-peer">{peerId}</strong>{known && <span className="request-known">Known device</span>}</div>
        </div>

        <div className="permission-summary" aria-label="Requested permissions">
          <div><span className="permission-check" aria-hidden="true">✓</span><span>See your screen</span></div>
          <div><span className="permission-check" aria-hidden="true">✓</span><span>Control mouse &amp; keyboard</span></div>
        </div>

        <label className="always-allow"><input type="checkbox" checked={alwaysChecked} onChange={(event) => setAlwaysChecked(event.target.checked)} /> <span>Always allow this device</span></label>
        <div className="incoming-actions">
          <button ref={rejectRef} className="ghost-button" type="button" disabled={busy} onClick={() => void resolve(false)}>Reject</button>
          <button className="primary-button incoming-allow" type="button" disabled={busy} onClick={() => void resolve(true)}>Allow</button>
        </div>
        <p className="incoming-footer">End-to-end encrypted — OpenDesk never sees your screen.</p>
      </section>
    </div>
  );
}
