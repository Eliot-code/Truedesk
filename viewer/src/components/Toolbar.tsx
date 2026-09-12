import { useEffect, useState, type ReactNode } from "react";

interface ToolbarProps {
  remoteId: string;
  latency?: string;
  onDisconnect: () => void;
}

function IconButton({ label, children, onClick }: { label: string; children: ReactNode; onClick?: () => void }) {
  return <button className="session-toolbar__icon" type="button" aria-label={label} onClick={onClick}>{children}</button>;
}

function formatDuration(seconds: number): string {
  const minutes = Math.floor(seconds / 60).toString().padStart(2, "0");
  return `${minutes}:${(seconds % 60).toString().padStart(2, "0")}`;
}

export default function Toolbar({ remoteId, latency = "18 ms", onDisconnect }: ToolbarProps) {
  const [elapsed, setElapsed] = useState(0);
  const [clipboard, setClipboard] = useState(false);
  const [keyboard, setKeyboard] = useState(false);

  useEffect(() => {
    const startedAt = Date.now();
    const timer = window.setInterval(() => setElapsed(Math.floor((Date.now() - startedAt) / 1000)), 1000);
    return () => window.clearInterval(timer);
  }, []);

  const toggleFullscreen = () => {
    if (document.fullscreenElement) void document.exitFullscreen();
    else void document.documentElement.requestFullscreen?.();
  };

  return (
    <>
      <div className="session-toolbar" role="toolbar" aria-label="Session controls">
        <div className="session-toolbar__connection">
          <span className="session-toolbar__connected-dot" aria-hidden="true" />
          <span className="session-toolbar__remote mono">{remoteId}</span>
          <span className="session-toolbar__latency mono">{latency}</span>
        </div>
        <div className="session-toolbar__controls">
          <IconButton label="Fullscreen" onClick={toggleFullscreen}>⛶</IconButton>
          <IconButton label="Fit / Scale">↗</IconButton>
          <label className="session-toolbar__select-chip">
            <span className="sr-only">Quality</span>
            <select aria-label="Quality" defaultValue="auto"><option value="auto">Auto</option><option value="high">High</option><option value="balanced">Balanced</option></select>
          </label>
          <IconButton label="Clipboard" onClick={() => setClipboard((value) => !value)}>{clipboard ? "▣" : "□"}</IconButton>
          <IconButton label="Keyboard" onClick={() => setKeyboard((value) => !value)}>{keyboard ? "⌨" : "⌑"}</IconButton>
        </div>
        <div className="session-toolbar__actions">
          <span className="session-toolbar__timer mono" aria-label="Session timer">{formatDuration(elapsed)}</span>
          <button className="session-toolbar__disconnect" type="button" onClick={onDisconnect}>Disconnect</button>
        </div>
      </div>
      <div className="session-security-chip mono">Encrypted · 60 fps</div>
    </>
  );
}

export type { ToolbarProps };
