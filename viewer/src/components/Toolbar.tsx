import { useEffect, useState, type ReactNode } from "react";

interface ToolbarProps {
  remoteId: string;
  latency?: string;
  clipboardSync?: boolean;
  onClipboardSyncChange?: (enabled: boolean) => void;
  onDisconnect: () => void;
  monitors?: import("../lib/api").MonitorInfo[];
  selectedMonitor?: number;
  onMonitorChange?: (id: number) => void;
  audioMuted?: boolean;
  onAudioMutedChange?: (muted: boolean) => void;
  recordingActive?: boolean;
  recordingElapsed?: number;
  recordingDisabled?: boolean;
  onRecordingToggle?: () => void;
}

function IconButton({ label, children, onClick, disabled = false, title }: { label: string; children: ReactNode; onClick?: () => void; disabled?: boolean; title?: string }) {
  return <button className="session-toolbar__icon" type="button" aria-label={label} onClick={onClick} disabled={disabled} title={title}>{children}</button>;
}

function formatDuration(seconds: number): string {
  const minutes = Math.floor(seconds / 60).toString().padStart(2, "0");
  return `${minutes}:${(seconds % 60).toString().padStart(2, "0")}`;
}

export default function Toolbar({ remoteId, latency, clipboardSync = true, onClipboardSyncChange = () => {}, onDisconnect, monitors = [], selectedMonitor, onMonitorChange = () => {}, audioMuted = false, onAudioMutedChange = () => {}, recordingActive = false, recordingElapsed = 0, recordingDisabled = false, onRecordingToggle = () => {} }: ToolbarProps) {
  const [elapsed, setElapsed] = useState(0);

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
          {latency && <span className="session-toolbar__latency mono">{latency}</span>}
        </div>
        <div className="session-toolbar__controls">
          <IconButton label="Fullscreen" onClick={toggleFullscreen}>⛶</IconButton>
          <IconButton label="Fit / Scale" disabled title="Not available yet">↗</IconButton>
          <label className="session-toolbar__select-chip">
            <span className="sr-only">Quality</span>
            <select aria-label="Quality" defaultValue="auto" disabled title="Not available yet"><option value="auto">Auto</option><option value="high">High</option><option value="balanced">Balanced</option></select>
          </label>
          {monitors.length > 0 && <label className="session-toolbar__select-chip">
            <span className="sr-only">Monitor</span>
            <select aria-label="Monitor" value={selectedMonitor ?? monitors[0].id} onChange={(event) => onMonitorChange(Number(event.target.value))}>
              {monitors.map((monitor) => <option key={monitor.id} value={monitor.id}>{monitor.name}{monitor.width && monitor.height ? ` · ${monitor.width}×${monitor.height}` : ""}{monitor.is_primary ? " · Primary" : ""}</option>)}
            </select>
          </label>}
          <IconButton label="Sync clipboard" onClick={() => onClipboardSyncChange(!clipboardSync)}>{clipboardSync ? "▣" : "□"}</IconButton>
          <IconButton label="Mute audio" onClick={() => onAudioMutedChange(!audioMuted)}>{audioMuted ? "🔇" : "🔊"}</IconButton>
          {recordingActive && <span className="session-toolbar__recording-status" aria-label="Recording elapsed"><span className="session-toolbar__recording-dot" aria-hidden="true" />{formatDuration(recordingElapsed)}</span>}
          <IconButton label={recordingActive ? "Stop recording" : "Record"} onClick={onRecordingToggle} disabled={recordingDisabled}>
            {recordingActive ? "■" : "●"}
          </IconButton>
          {recordingDisabled && <span className="sr-only">Recording is only available while viewing a session</span>}
          <IconButton label="Keyboard" disabled title="Not available yet">⌑</IconButton>
        </div>
        <div className="session-toolbar__actions">
          <span className="session-toolbar__timer mono" aria-label="Session timer">{formatDuration(elapsed)}</span>
          <button className="session-toolbar__disconnect" type="button" onClick={onDisconnect}>Disconnect</button>
        </div>
      </div>
      <div className="session-security-chip mono">Encrypted</div>
    </>
  );
}

export type { ToolbarProps };
