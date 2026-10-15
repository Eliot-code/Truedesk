import { useEffect, useRef, useState, type FormEvent, type KeyboardEvent } from "react";

export interface TwofaPromptProps {
  /** "required": the host asked for a code. "invalid": the last code was rejected. */
  reason: "required" | "invalid";
  /** Disables the controls while a reconnect is in flight. */
  busy?: boolean;
  onSubmit(code: string): void;
  onCancel(): void;
}

/**
 * Asks for the host's 6-digit two-factor code after a connection is rejected
 * for one. The desktop viewer shows it over the home screen.
 */
export default function TwofaPrompt({ reason, busy = false, onSubmit, onCancel }: TwofaPromptProps) {
  const [code, setCode] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => { inputRef.current?.focus(); }, []);

  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (!/^\d{6}$/.test(code)) return;
    onSubmit(code);
  };

  const onKeyDown = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key === "Escape") { event.preventDefault(); onCancel(); }
  };

  return (
    <div className="incoming-backdrop" role="presentation">
      <section className="incoming-modal twofa-modal" role="dialog" aria-modal="true" aria-labelledby="twofa-prompt-title" onKeyDown={onKeyDown}>
        <div className="incoming-icon" aria-hidden="true">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7">
            <rect x="4" y="10" width="16" height="10" rx="2" />
            <path d="M8 10V7a4 4 0 0 1 8 0v3" />
          </svg>
        </div>
        <span className="eyebrow">TWO-FACTOR REQUIRED</span>
        <h1 id="twofa-prompt-title">Enter your 2FA code</h1>
        <p className="incoming-lead">
          {reason === "invalid"
            ? "That code didn't match. Enter the current 6-digit code from your authenticator."
            : "This device asks for a two-factor code. Enter the current 6-digit code from your authenticator app."}
        </p>
        <form onSubmit={submit}>
          <input
            ref={inputRef}
            className="remote-password-input mono twofa-prompt-input"
            inputMode="numeric"
            autoComplete="one-time-code"
            maxLength={6}
            pattern="[0-9]{6}"
            placeholder="000000"
            aria-label="Two-factor code"
            value={code}
            onChange={(event) => setCode(event.target.value.replace(/\D/g, ""))}
          />
          <div className="incoming-actions">
            <button className="ghost-button" type="button" disabled={busy} onClick={onCancel}>Cancel</button>
            <button className="primary-button" type="submit" disabled={busy || code.length !== 6}>{busy ? "Connecting…" : "Submit"}</button>
          </div>
        </form>
      </section>
    </div>
  );
}
