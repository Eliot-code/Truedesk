import { useEffect } from "react";
import type { Notice } from "../lib/notify";

/** How long an informational notice stays up. Errors stay until dismissed. */
export const INFO_TIMEOUT_MS = 4000;

export interface ToastProps { notice: Notice | null; onDismiss: () => void }

export default function Toast({ notice, onDismiss }: ToastProps) {
  useEffect(() => {
    if (!notice || notice.tone !== "info") return;
    const timer = window.setTimeout(onDismiss, INFO_TIMEOUT_MS);
    return () => window.clearTimeout(timer);
  }, [notice, onDismiss]);

  if (!notice) return null;
  return (
    <div className={`toast toast--${notice.tone}`} role={notice.tone === "error" ? "alert" : "status"}>
      <span className="toast__text">{notice.text}</span>
      <button className="toast__dismiss" type="button" onClick={onDismiss} aria-label="Dismiss">×</button>
    </div>
  );
}
