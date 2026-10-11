import { useEffect, useRef } from "react";

export interface ConnectingProps { remoteId: string; onCancel: () => void }

/** "123456789" -> "123 456 789", matching the ID input placeholder. */
export function formatId(id: string): string {
  return /^\d{9}$/.test(id) ? id.replace(/(\d{3})(?=\d)/g, "$1 ") : id;
}

export default function Connecting({ remoteId, onCancel }: ConnectingProps) {
  const cancelRef = useRef<HTMLButtonElement>(null);
  useEffect(() => { cancelRef.current?.focus(); }, []);

  return (
    <main className="connecting-screen">
      <section className="connecting-card" aria-live="polite" aria-busy="true">
        <span className="connecting-spinner" aria-hidden="true" />
        <span className="eyebrow">CONNECTING…</span>
        <h1>Waiting for <span className="mono">{formatId(remoteId)}</span> to accept</h1>
        <p>The person on the other device needs to allow the request.</p>
        <button ref={cancelRef} className="ghost-button connecting-cancel" type="button" onClick={onCancel}>Cancel</button>
      </section>
    </main>
  );
}
