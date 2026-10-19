import { useCallback, useEffect, useRef, useState } from "react";
import {
  host,
  initialShareState,
  parseHostEvent,
  reduceHostEvent,
  type ShareState,
} from "../lib/host";
import { formatId, hostId, hostPassword, hostToken, resetHostId, savedRelay, setHostPassword } from "../lib/settings";

export interface ShareScreenProps {
  onBack(): void;
}

const PHASE_LABEL: Record<ShareState["phase"], string> = {
  idle: "Not sharing",
  starting: "Starting…",
  connecting: "Connecting to the relay…",
  online: "Ready — waiting for a viewer",
  in_session: "A viewer is connected",
};

export default function ShareScreen({ onBack }: ShareScreenProps) {
  const [id, setId] = useState(hostId);
  const [password, setPassword] = useState(hostPassword);
  const [share, setShare] = useState<ShareState>(initialShareState);
  const [accessibilityOn, setAccessibilityOn] = useState(true);
  const [needAccessibility, setNeedAccessibility] = useState(false);
  const polling = useRef(false);

  const sharing = share.phase !== "idle" && share.phase !== "starting";

  const stop = useCallback(async () => {
    polling.current = false;
    try {
      await host.stop();
    } catch {
      // Already stopped; nothing to do.
    }
    setShare(initialShareState);
  }, []);

  // Stop sharing if the screen goes away.
  useEffect(() => () => { polling.current = false; void host.stop().catch(() => {}); }, []);

  // Reflect whether the accessibility service is on when we arrive and on return.
  useEffect(() => {
    let alive = true;
    const check = () => host.state().then((state) => { if (alive) setAccessibilityOn(state.accessibilityEnabled); }).catch(() => {});
    check();
    const onVisible = () => { if (document.visibilityState === "visible") check(); };
    document.addEventListener("visibilitychange", onVisible);
    return () => { alive = false; document.removeEventListener("visibilitychange", onVisible); };
  }, []);

  const pump = useCallback(async () => {
    while (polling.current) {
      let line: string | null | undefined;
      try {
        ({ event: line } = await host.poll(1000));
      } catch {
        break;
      }
      if (!polling.current) break;
      if (!line) continue;
      const event = parseHostEvent(line);
      if (!event) continue;
      if (event.t === "identity_in_use") {
        polling.current = false;
        await host.stop().catch(() => {});
        setId(resetHostId());
        setShare({ ...initialShareState, error: "That device ID was taken; a new one was chosen. Start again." });
        return;
      }
      setShare((state) => reduceHostEvent(state, event));
    }
  }, []);

  const start = useCallback(async () => {
    setShare({ ...initialShareState, phase: "starting" });
    setNeedAccessibility(false);
    let result;
    try {
      result = await host.start({ relays: [savedRelay()], id, token: hostToken(), password });
    } catch (error) {
      setShare({ ...initialShareState, error: error instanceof Error ? error.message : String(error) });
      return;
    }
    if (!result.started) {
      if (result.reason === "accessibility") {
        setNeedAccessibility(true);
        setAccessibilityOn(false);
      }
      const message =
        result.reason === "accessibility" ? "Turn on Truedesk remote control first." :
        result.reason === "projection_denied" ? "Screen sharing was not allowed." :
        "Couldn't start sharing.";
      setShare({ ...initialShareState, error: message });
      return;
    }
    setShare({ ...initialShareState, phase: "connecting" });
    polling.current = true;
    void pump();
  }, [id, password, pump]);

  const changePassword = (value: string) => {
    setPassword(value);
    setHostPassword(value);
  };

  return (
    <main className="app-shell web-shell">
      <header className="topbar">
        <button className="ghost-button" onClick={onBack}>&larr; Back</button>
        <div className="status mono">SHARE THIS DEVICE</div>
      </header>
      <section className="hero web-hero">
        <span className="eyebrow">HOST</span>
        <h1>Let someone control this device.</h1>
        <p>Give the ID and password below to the person connecting from Truedesk. They see your screen and can tap and type here while you share.</p>
      </section>

      <section className="card connect-card web-connect-card">
        {share.error && <p className="message" role="alert">{share.error}</p>}

        <div className="share-identity">
          <div className="share-field">
            <span className="caption">DEVICE ID</span>
            <p className="share-id mono">{formatId(id)}</p>
          </div>
          <div className="share-field">
            <span className="caption">PASSWORD</span>
            <input
              className="remote-password-input mono"
              aria-label="Password viewers must enter"
              value={password}
              onChange={(event) => changePassword(event.target.value)}
              disabled={sharing}
            />
          </div>
        </div>

        <p className="status share-status" role="status">{PHASE_LABEL[share.phase]}</p>

        {!accessibilityOn && (
          <div className="share-step">
            <p>Remote control needs the Truedesk accessibility service, so taps and typing work.</p>
            <button className="ghost-button" onClick={() => void host.openAccessibilitySettings()}>
              {needAccessibility ? "Open Accessibility settings" : "Turn on remote control"}
            </button>
          </div>
        )}

        {!sharing ? (
          <button className="primary-button" onClick={() => void start()} disabled={share.phase === "starting"}>
            {share.phase === "starting" ? "Starting…" : "Start sharing"}
          </button>
        ) : (
          <button className="ghost-button share-stop" onClick={() => void stop()}>Stop sharing</button>
        )}

        <p className="caption web-reassure">
          Everything stays encrypted to the connecting device; the relay only passes sealed data through. Sharing shows an ongoing notification and stops the moment you tap Stop.
        </p>
      </section>

      {share.request && (
        <div className="incoming-backdrop" role="presentation">
          <section className="incoming-modal" role="dialog" aria-modal="true" aria-label="Allow this viewer?">
            <span className="eyebrow">INCOMING REQUEST</span>
            <h1>Allow this viewer?</h1>
            <p className="incoming-lead">Device <span className="mono">{formatId(share.request)}</span> wants to see and control this device.</p>
            <div className="incoming-actions">
              <button className="ghost-button" onClick={() => { void host.answer(false); setShare((s) => ({ ...s, request: null })); }}>Deny</button>
              <button className="primary-button" onClick={() => { void host.answer(true); setShare((s) => ({ ...s, request: null })); }}>Allow</button>
            </div>
          </section>
        </div>
      )}
    </main>
  );
}
