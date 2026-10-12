import { useCallback, useEffect, useRef, useState } from "react";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

/** How often a running app looks for a new release. */
export const CHECK_INTERVAL_MS = 6 * 60 * 60 * 1000;

type Phase = "available" | "downloading" | "ready" | "installing" | "failed";

export interface UpdateBannerProps {
  /** False in dev builds, where there is nothing to update. */
  enabled: boolean;
  /** Only shown on the home screen; a session is never interrupted. */
  visible: boolean;
}

/**
 * Checks for a signed release in the background and offers to install it.
 * The updater plugin verifies the signature before installing. Failures are
 * logged and otherwise ignored: an update is never worth a scary dialog.
 */
export default function UpdateBanner({ enabled, visible }: UpdateBannerProps) {
  const [update, setUpdate] = useState<Update | null>(null);
  const [phase, setPhase] = useState<Phase>("available");
  const [progress, setProgress] = useState<number | null>(null);
  const updateRef = useRef<Update | null>(null);
  const dismissed = useRef<string | null>(null);
  const visibleRef = useRef(visible);
  visibleRef.current = visible;

  useEffect(() => {
    if (!enabled) return;
    let active = true;
    const run = async () => {
      if (updateRef.current) return;
      try {
        const found = await check();
        if (!found) return;
        if (!active || found.version === dismissed.current) { void found.close().catch(() => {}); return; }
        updateRef.current = found;
        setUpdate(found);
        setPhase("available");
      } catch (error) {
        console.warn("Update check failed:", error);
      }
    };
    void run();
    const timer = window.setInterval(() => void run(), CHECK_INTERVAL_MS);
    return () => { active = false; window.clearInterval(timer); };
  }, [enabled]);

  const restart = useCallback(async (target: Update) => {
    setPhase("installing");
    // On Windows, install() hands over to the installer and exits the app.
    await target.install();
    await relaunch();
  }, []);

  const start = useCallback(async () => {
    const target = updateRef.current;
    if (!target) return;
    try {
      if (phase === "ready") { await restart(target); return; }
      setPhase("downloading");
      setProgress(null);
      let total = 0;
      let received = 0;
      await target.download((event) => {
        if (event.event === "Started") total = event.data.contentLength ?? 0;
        else if (event.event === "Progress" && total > 0) {
          received += event.data.chunkLength;
          setProgress(Math.min(100, Math.round((received / total) * 100)));
        }
      });
      // A session may have started while downloading; restart once it ends.
      if (!visibleRef.current) { setPhase("ready"); return; }
      await restart(target);
    } catch (error) {
      console.warn("Update install failed:", error);
      setPhase("failed");
    }
  }, [phase, restart]);

  const dismiss = () => {
    const target = updateRef.current;
    if (!target) return;
    dismissed.current = target.version;
    updateRef.current = null;
    setUpdate(null);
    void target.close().catch(() => {});
  };

  if (!update || !visible) return null;
  const busy = phase === "downloading" || phase === "installing";
  let text = `Update available: v${update.version}`;
  if (phase === "downloading") text = progress === null ? "Downloading update…" : `Downloading update… ${progress}%`;
  else if (phase === "installing") text = "Installing update…";
  else if (phase === "ready") text = `v${update.version} is ready to install.`;
  else if (phase === "failed") text = "The update didn't install. Try again later.";
  const action = phase === "failed" ? "Try again" : phase === "ready" ? "Restart to update" : "Install and restart";

  return (
    <div className="update-banner" role="status">
      <span className="update-banner__text">{text}</span>
      {!busy && <button className="update-banner__action" type="button" onClick={() => void start()}>{action}</button>}
      {!busy && <button className="toast__dismiss" type="button" onClick={dismiss} aria-label="Dismiss update">×</button>}
    </div>
  );
}
