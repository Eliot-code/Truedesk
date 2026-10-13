import { useEffect, useRef, useState, type ReactNode } from "react";
import { makeDecoder } from "../../../viewer/src/lib/decode";
import { attachInput } from "../../../viewer/src/lib/input";
import type { ConnectionStatus } from "../lib/connection";
import { formatId } from "../lib/settings";
import type { MonitorInfo } from "../lib/types";

export interface SessionControls {
  sendInput(event: Uint8Array): void;
  requestKeyframe(): void;
  selectMonitor(id: number): void;
  /** Where video frames go; the session screen sets it while mounted. */
  setVideoSink(sink: ((frame: Uint8Array) => void) | null): void;
}

export interface SessionProps {
  remoteId: string;
  status: ConnectionStatus;
  monitors: MonitorInfo[];
  controls: SessionControls;
  onDisconnect(): void;
}

const overlays: Partial<Record<ConnectionStatus, { title: (id: string) => ReactNode; text: string }>> = {
  connecting: { title: (id) => <>Connecting to <span className="mono">{id}</span></>, text: "Checking the password with the other computer." },
  waiting: { title: (id) => <>Waiting for <span className="mono">{id}</span> to accept</>, text: "The person on the other device needs to allow the request." },
  reconnecting: { title: () => "Connection lost", text: "Reconnecting…" },
};

const statusDot: Record<ConnectionStatus, string> = { connecting: "connecting", waiting: "connecting", reconnecting: "connecting", streaming: "in_session" };

function monitorName(monitor: MonitorInfo, index: number): string {
  const device = /^\\\\\.\\DISPLAY(\d+)$/i.exec(monitor.name.trim());
  const name = device ? `Display ${device[1]}` : monitor.name.trim() || `Display ${index + 1}`;
  return `${name}${monitor.width && monitor.height ? ` · ${monitor.width}×${monitor.height}` : ""}${monitor.is_primary ? " · Primary" : ""}`;
}

export default function Session({ remoteId, status, monitors, controls, onDisconnect }: SessionProps) {
  const screenRef = useRef<HTMLElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [selected, setSelected] = useState<number>();

  useEffect(() => {
    setSelected(monitors.find((monitor) => monitor.is_primary)?.id ?? monitors[0]?.id);
  }, [monitors]);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const decoder = makeDecoder(canvas, { requestKeyframe: () => controls.requestKeyframe() });
    controls.setVideoSink(decoder.push);
    const detachInput = attachInput(canvas, async (bytes) => controls.sendInput(bytes));
    // Right clicks go to the remote computer, not the browser menu.
    const noMenu = (event: Event) => event.preventDefault();
    canvas.addEventListener("contextmenu", noMenu);
    canvas.focus();
    return () => {
      controls.setVideoSink(null);
      detachInput();
      canvas.removeEventListener("contextmenu", noMenu);
      decoder.close();
    };
  }, [controls]);

  const toggleFullscreen = () => {
    if (document.fullscreenElement) void document.exitFullscreen();
    else void screenRef.current?.requestFullscreen?.().then(() => canvasRef.current?.focus());
  };

  const chooseMonitor = (id: number) => {
    setSelected(id);
    controls.selectMonitor(id);
  };

  const overlay = overlays[status];

  return (
    <main ref={screenRef} className="session-screen">
      <canvas ref={canvasRef} className="session-canvas" tabIndex={0} aria-label="Remote screen" />
      {overlay && (
        <div className="session-host-overlay web-overlay" aria-live="polite">
          <span className="connecting-spinner" aria-hidden="true" />
          <h1>{overlay.title(formatId(remoteId))}</h1>
          <p>{overlay.text}</p>
        </div>
      )}
      <div className="session-toolbar" role="toolbar" aria-label="Session controls">
        <div className="session-toolbar__connection">
          <span className="session-toolbar__connected-dot" data-status={statusDot[status]} aria-hidden="true" />
          <span className="sr-only">{status === "streaming" ? "Connected" : "Connecting"}</span>
          <span className="session-toolbar__remote mono">{formatId(remoteId)}</span>
        </div>
        <div className="session-toolbar__controls">
          <button className="session-toolbar__icon" type="button" aria-label="Fullscreen" title="Fullscreen" onClick={toggleFullscreen}>
            <svg className="session-toolbar__svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <path d="M8 3H5a2 2 0 0 0-2 2v3" /><path d="M21 8V5a2 2 0 0 0-2-2h-3" /><path d="M3 16v3a2 2 0 0 0 2 2h3" /><path d="M16 21h3a2 2 0 0 0 2-2v-3" />
            </svg>
            <span className="session-toolbar__icon-text" aria-hidden="true">Fullscreen</span>
          </button>
          {monitors.length > 1 && (
            <label className="session-toolbar__select-chip">
              <select aria-label="Monitor" value={selected ?? monitors[0].id} onChange={(event) => chooseMonitor(Number(event.target.value))}>
                {monitors.map((monitor, index) => <option key={monitor.id} value={monitor.id}>{monitorName(monitor, index)}</option>)}
              </select>
            </label>
          )}
        </div>
        <div className="session-toolbar__actions">
          <button className="session-toolbar__disconnect" type="button" onClick={onDisconnect}>Disconnect</button>
        </div>
      </div>
      <div className="session-security-chip mono">Encrypted</div>
    </main>
  );
}
