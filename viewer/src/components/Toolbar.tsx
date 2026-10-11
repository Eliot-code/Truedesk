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

interface IconButtonProps { label: string; text: string; icon: ReactNode; onClick?: () => void; title?: string; pressed?: boolean; expanded?: boolean; tone?: "danger" }

function IconButton({ label, text, icon, onClick, title, pressed, expanded, tone }: IconButtonProps) {
  return (
    <button className="session-toolbar__icon" type="button" aria-label={label} aria-pressed={pressed} aria-expanded={expanded} data-tone={tone} onClick={onClick} title={title ?? label}>
      {icon}
      <span className="session-toolbar__icon-text" aria-hidden="true">{text}</span>
    </button>
  );
}

// Icon paths from Lucide (https://lucide.dev, ISC License).
function Icon({ children }: { children: ReactNode }) {
  return <svg className="session-toolbar__svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">{children}</svg>;
}

const speaker = <path d="M11 4.702a.705.705 0 0 0-1.203-.498L6.413 7.587A1.4 1.4 0 0 1 5.416 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.705.705 0 0 0 11 19.298z" />;
const icons = {
  fullscreen: <Icon><path d="M8 3H5a2 2 0 0 0-2 2v3" /><path d="M21 8V5a2 2 0 0 0-2-2h-3" /><path d="M3 16v3a2 2 0 0 0 2 2h3" /><path d="M16 21h3a2 2 0 0 0 2-2v-3" /></Icon>,
  monitor: <Icon><rect width="20" height="14" x="2" y="3" rx="2" /><line x1="8" x2="16" y1="21" y2="21" /><line x1="12" x2="12" y1="17" y2="21" /></Icon>,
  clipboard: <Icon><rect width="8" height="4" x="8" y="2" rx="1" /><path d="M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2" /></Icon>,
  sound: <Icon>{speaker}<path d="M16 9a5 5 0 0 1 0 6" /><path d="M19.364 18.364a9 9 0 0 0 0-12.728" /></Icon>,
  muted: <Icon>{speaker}<line x1="22" x2="16" y1="9" y2="15" /><line x1="16" x2="22" y1="9" y2="15" /></Icon>,
  files: <Icon><path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z" /></Icon>,
  record: <Icon><circle cx="12" cy="12" r="7" fill="currentColor" stroke="none" /></Icon>,
  stop: <Icon><rect x="6" y="6" width="12" height="12" rx="2" fill="currentColor" stroke="none" /></Icon>,
};

/** Readable monitor name: Windows reports device paths like `\\.\DISPLAY2`. */
export function monitorName(name: string, index: number): string {
  const device = /^\\\\\.\\DISPLAY(\d+)$/i.exec(name.trim());
  if (device) return `Display ${device[1]}`;
  return name.trim() || `Display ${index + 1}`;
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
  const clipboard = <IconButton label="Sync clipboard" pressed={clipboardSync} title={clipboardSync ? "Clipboard sync is on. Click to turn it off." : "Clipboard sync is off. Click to turn it on."} onClick={() => onClipboardSyncChange(!clipboardSync)}text="Clipboard" icon={icons.clipboard} />;

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
          {viewer && <IconButton label="Fullscreen" text="Fullscreen" icon={icons.fullscreen} onClick={toggleFullscreen} />}
          {viewer && monitors.length > 0 && <label className="session-toolbar__select-chip">
            <span className="sr-only">Monitor</span>
            {icons.monitor}
            <select aria-label="Monitor" value={selectedMonitor ?? monitors[0].id} onChange={(event) => onMonitorChange(Number(event.target.value))}>
              {monitors.map((monitor, index) => <option key={monitor.id} value={monitor.id}>{monitorName(monitor.name, index)}{monitor.width && monitor.height ? ` · ${monitor.width}×${monitor.height}` : ""}{monitor.is_primary ? " · Primary" : ""}</option>)}
            </select>
          </label>}
          {clipboard}
          {viewer && <>
            <IconButton label="Mute audio" pressed={audioMuted} title={audioMuted ? "Audio is muted. Click to unmute." : "Audio is on. Click to mute."} onClick={() => onAudioMutedChange(!audioMuted)} text={audioMuted ? "Muted" : "Sound"} icon={audioMuted ? icons.muted : icons.sound} />
            <IconButton label="Files" pressed={filesOpen} expanded={filesOpen} title={filesOpen ? "Hide file transfer" : "Send or receive files"} onClick={onFilesToggle} text="Files" icon={icons.files} />
            {recordingActive && <span className="session-toolbar__recording-status" aria-label="Recording elapsed"><span className="session-toolbar__recording-dot" aria-hidden="true" />{formatDuration(recordingElapsed)}</span>}
            <IconButton label={recordingActive ? "Stop recording" : "Record"} text={recordingActive ? "Stop" : "Record"} icon={recordingActive ? icons.stop : icons.record} tone="danger" onClick={onRecordingToggle} />
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
