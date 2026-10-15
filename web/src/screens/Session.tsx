import { useEffect, useRef, useState, type ReactNode } from "react";
import { makeDecoder } from "../../../viewer/src/lib/decode";
import { makeAudioPlayer } from "../lib/audio";
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
  /** Where audio packets go while sound is on. */
  setAudioSink(sink: ((payload: Uint8Array, channels: number) => void) | null): void;
  /** Send local clipboard text to the host. */
  sendClipboard(text: string): void;
  /** Where inbound clipboard text goes while the sync toggle is on. */
  setClipboardSink(sink: ((text: string) => void) | null): void;
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
  const [clipboardOn, setClipboardOn] = useState(false);
  const [soundOn, setSoundOn] = useState(false);

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

  // Clipboard sync is off by default, like the desktop viewer. While on,
  // inbound text is written to the page clipboard and local text is sent back
  // on focus. The same value is never echoed straight back to its origin.
  useEffect(() => {
    if (!clipboardOn) {
      controls.setClipboardSink(null);
      return;
    }
    const clip = navigator.clipboard;
    let last = "";
    controls.setClipboardSink((text) => {
      last = text;
      void clip?.writeText?.(text).catch(() => {});
    });
    const pull = () => {
      void clip?.readText?.().then((text) => {
        if (text && text !== last && text.length <= 1_000_000) {
          last = text;
          controls.sendClipboard(text);
        }
      }).catch(() => {});
    };
    window.addEventListener("focus", pull);
    pull();
    return () => {
      controls.setClipboardSink(null);
      window.removeEventListener("focus", pull);
    };
  }, [clipboardOn, controls]);

  // Sound is off by default. Toggling it on happens inside a click (a user
  // gesture), so the AudioContext is allowed to start. If WebCodecs audio is
  // missing, makeAudioPlayer returns null and we simply play nothing.
  useEffect(() => {
    if (!soundOn) {
      controls.setAudioSink(null);
      return;
    }
    const player = makeAudioPlayer();
    if (!player) {
      controls.setAudioSink(null);
      return;
    }
    controls.setAudioSink((payload, channels) => player.push(payload, channels));
    return () => {
      controls.setAudioSink(null);
      player.close();
    };
  }, [soundOn, controls]);

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
    <main ref={screenRef} className="session-screen web-session">
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
          <button className="session-toolbar__icon" type="button" aria-pressed={clipboardOn} aria-label="Clipboard sync" title={clipboardOn ? "Clipboard sync on" : "Clipboard sync off"} onClick={() => setClipboardOn((on) => !on)}>
            <svg className="session-toolbar__svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <rect x="8" y="2" width="8" height="4" rx="1" /><path d="M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2" />
            </svg>
            <span className="session-toolbar__icon-text" aria-hidden="true">Clipboard</span>
          </button>
          <button className="session-toolbar__icon" type="button" aria-pressed={soundOn} aria-label="Sound" title={soundOn ? "Sound on" : "Sound off"} onClick={() => setSoundOn((on) => !on)}>
            <svg className="session-toolbar__svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <path d="M11 5 6 9H2v6h4l5 4V5z" />
              {soundOn ? <><path d="M15.5 8.5a5 5 0 0 1 0 7" /><path d="M18.5 5.5a9 9 0 0 1 0 13" /></> : <path d="M22 9l-6 6M16 9l6 6" />}
            </svg>
            <span className="session-toolbar__icon-text" aria-hidden="true">Sound</span>
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
