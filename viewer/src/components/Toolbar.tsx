import { useEffect, useState, type ReactNode } from "react";
import type { MonitorInfo, Status } from "../lib/api";

interface ToolbarProps {
  remoteId: string;
  /** "host" shows only the timer, clipboard toggle and Disconnect. */
  role?: "host" | "viewer";
  status?: Status;
  latency?: string;
  clipboardSync?: boolean;
  onClipboardSyncChange?: (enabled: boolean) => void;
  onDisconnect: () => void;
  monitors?: MonitorInfo[];
  selectedMonitor?: number;
  onMonitorChange?: (id: number) => void;
  audioMuted?: boolean;
  onAudioMutedChange?: (muted: boolean) => void;
  recordingActive?: boolean;
  recordingElapsed?: number;
  onRecordingToggle?: () => void;
  filesOpen?: boolean;
  onFilesToggle?: () => void;
}

interface IconButtonProps { label: string; children: ReactNode; onClick?: () => void; title?: string; pressed?: boolean; expanded?: boolean }

function IconButton({ label, children, onClick, title, pressed, expanded }: IconButtonProps) {
  return <button className="session-toolbar__icon" type="button" aria-label={label} aria-pressed={pressed} aria-expanded={expanded} onClick={onClick} title={title ?? label}>{children}</button>;
}

function formatDuration(seconds: number): string {
  const minutes = Math.floor(seconds / 60).toString().padStart(2, "0");
  return `${minutes}:${(seconds % 60).toString().padStart(2, "0")}`;
}

const connectionLabels: Record<Status, string> = { in_session: "Connected", connecting: "Reconnecting", online: "Connected", offline: "Disconnected" };

export default function Toolbar({ remoteId, role = "viewer", status = "in_session", latency, clipboardSync = true, onClipboardSyncChange = () => {}, onDisconnect, monitors = [], selectedMonitor, onMonitorChange = () => {}, audioMuted = false, onAudioMutedChange = () => {}, recordingActive = false, recordingElapsed = 0, onRecordingToggle = () => {}, filesOpen = false, onFilesToggle = () => {} }: ToolbarProps) {
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

  const viewer = role === "viewer";
  const clipboard = <IconButton label="Sync clipboard" pressed={clipboardSync} title={clipboardSync ? "Clipboard sync is on. Click to turn it off." : "Clipboard sync is off. Click to turn it on."} onClick={() => onClipboardSyncChange(!clipboardSync)}>{clipboardSync ? "▣" : "□"}</IconButton>;

  return (
    <>
      <div className="session-toolbar" role="toolbar" aria-label="Session controls">
        {viewer && <div className="session-toolbar__connection">
          <span className="session-toolbar__connected-dot" data-status={status} title={connectionLabels[status]} aria-hidden="true" />
          <span className="sr-only">{connectionLabels[status]}</span>
          <span className="session-toolbar__remote mono">{remoteId}</span>
          {latency && <span className="session-toolbar__latency mono">{latency}</span>}
        </div>}
        <div className="session-toolbar__controls">
          {viewer && <IconButton label="Fullscreen" onClick={toggleFullscreen}>⛶</IconButton>}
          {viewer && monitors.length > 0 && <label className="session-toolbar__select-chip">
            <span className="sr-only">Monitor</span>
            <select aria-label="Monitor" value={selectedMonitor ?? monitors[0].id} onChange={(event) => onMonitorChange(Number(event.target.value))}>
              {monitors.map((monitor) => <option key={monitor.id} value={monitor.id}>{monitor.name}{monitor.width && monitor.height ? ` · ${monitor.width}×${monitor.height}` : ""}{monitor.is_primary ? " · Primary" : ""}</option>)}
            </select>
          </label>}
          {clipboard}
          {viewer && <>
            <IconButton label="Mute audio" pressed={audioMuted} title={audioMuted ? "Audio is muted. Click to unmute." : "Audio is on. Click to mute."} onClick={() => onAudioMutedChange(!audioMuted)}>{audioMuted ? "🔇" : "🔊"}</IconButton>
            <IconButton label="Files" pressed={filesOpen} expanded={filesOpen} title={filesOpen ? "Hide file transfer" : "Send or receive files"} onClick={onFilesToggle}>⇅</IconButton>
            {recordingActive && <span className="session-toolbar__recording-status" aria-label="Recording elapsed"><span className="session-toolbar__recording-dot" aria-hidden="true" />{formatDuration(recordingElapsed)}</span>}
            <IconButton label={recordingActive ? "Stop recording" : "Record"} onClick={onRecordingToggle}>{recordingActive ? "■" : "●"}</IconButton>
          </>}
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
