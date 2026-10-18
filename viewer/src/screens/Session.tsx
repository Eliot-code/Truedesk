import { useCallback, useEffect, useRef, useState } from "react";
import * as api from "../lib/api";
import { makeDecoder } from "../lib/decode";
import { attachInput } from "../lib/input";
import Toolbar, { monitorName, virtualDisplaySize } from "../components/Toolbar";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import FileTransfer from "../components/FileTransfer";
import ChatPanel, { type ChatEntry } from "../components/ChatPanel";
import HostControlPanel from "../components/HostControlPanel";
import TunnelPanel, { tunnelRefusal } from "../components/TunnelPanel";
import { friendlyError, useNotify } from "../lib/notify";

/** Why the host didn't add or remove a virtual display. */
export function virtualDisplayRefusal(code: string): string {
  switch (code) {
    case "limit": return "This session already has as many virtual displays as it can add. Remove one first.";
    case "not_allowed": return "The other computer doesn't allow adding virtual displays.";
    case "failed": return "The other computer couldn't add a virtual display.";
    case "unknown_display": return "That virtual display is already gone.";
    case "not_newest": return "Remove the newest virtual display first: Windows takes them away in reverse order.";
    default: return `The other computer couldn't change its displays (${code}).`;
  }
}

/** Decode a base64 IPC payload into bytes (audio arrives as base64, not number[]). */
function b64ToBytes(b64: string): Uint8Array {
  const binary = atob(b64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

export interface SessionProps {
  remoteId: string;
  role: "host" | "viewer";
  status?: api.Status;
  onDisconnect: () => void;
}

/**
 * Open (or focus) a separate window showing the host's monitor `id`. The
 * window runs the viewer app in monitor mode (see main.tsx) and owns its own
 * aux stream.
 */
async function openInNewWindow(id: number, name: string): Promise<void> {
  const label = `monitor-${id}`;
  const existing = await WebviewWindow.getByLabel(label);
  if (existing) { await existing.setFocus(); return; }
  const url = `index.html?monitor=${id}&label=${encodeURIComponent(name)}`;
  const window = new WebviewWindow(label, { url, title: `${name} · Truedesk`, width: 1280, height: 800 });
  await new Promise<void>((resolve, reject) => {
    void window.once("tauri://created", () => resolve());
    void window.once("tauri://error", (event) => reject(new Error(String(event.payload))));
  });
}

export default function Session({ remoteId, role, status = "in_session", onDisconnect }: SessionProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const notify = useNotify();
  const [filesOpen, setFilesOpen] = useState(false);
  const onFilesOpenChange = useCallback((open: boolean) => setFilesOpen(open), []);
  const [chatOpen, setChatOpen] = useState(false);
  const [messages, setMessages] = useState<ChatEntry[]>([]);
  const [chatUnread, setChatUnread] = useState(0);
  const chatOpenRef = useRef(false);
  chatOpenRef.current = chatOpen;
  const [viewOnly, setViewOnly] = useState(false);
  const viewOnlyRef = useRef(false);
  useEffect(() => { viewOnlyRef.current = viewOnly; }, [viewOnly]);
  const [canSecureAttention, setCanSecureAttention] = useState(false);
  const [canRestart, setCanRestart] = useState(false);
  const [restarting, setRestarting] = useState(false);
  const [canPrivacy, setCanPrivacy] = useState(false);
  const [privacyOn, setPrivacyOn] = useState(false);
  const [canTunnel, setCanTunnel] = useState(false);
  const [tunnelsOpen, setTunnelsOpen] = useState(false);
  // Off until the saved preference loads (clipboard sync defaults off).
  const [clipboardSync, setClipboardSync] = useState(false);
  const [audioMuted, setAudioMuted] = useState(false);
  const audioRef = useRef<{ context: AudioContext; nextTime: number } | undefined>(undefined);
  const recorderRef = useRef<MediaRecorder | undefined>(undefined);
  const recordingStreamRef = useRef<MediaStream | undefined>(undefined);
  const [recordingActive, setRecordingActive] = useState(false);
  const recordingStartingRef = useRef(false);
  const [recordingElapsed, setRecordingElapsed] = useState(0);
  const recordingStartedAtRef = useRef<number | undefined>(undefined);
  const [monitors, setMonitors] = useState<api.MonitorInfo[]>([]);
  const [selectedMonitor, setSelectedMonitor] = useState<number>();
  const [multiMonitorMax, setMultiMonitorMax] = useState<number>();
  const [link, setLink] = useState<api.LinkStats>();
  // Set once the host says it offers virtual displays (older hosts never do).
  const [virtualMax, setVirtualMax] = useState<number>();
  const toggleClipboardSync = (enabled: boolean) => {
    setClipboardSync(enabled);
    void api.setClipboardSync(enabled).catch((error: unknown) => { setClipboardSync(!enabled); notify(friendlyError(error)); });
  };

  useEffect(() => {
    let mounted = true;
    void api.getConfig().then((config) => { if (mounted) setClipboardSync(config.clipboard_sync); }).catch(() => {});
    return () => { mounted = false; };
  }, []);

  // Append inbound peer messages; count them as unread while the panel is closed.
  useEffect(() => {
    let stop: (() => void) | undefined;
    const listener = api.onChatMessage(({ text }) => {
      setMessages((current) => [...current, { from: "peer", text, at: Date.now() }]);
      if (!chatOpenRef.current) setChatUnread((count) => count + 1);
    });
    void listener.then((unlisten) => { stop = unlisten; });
    return () => { if (stop) stop(); else void listener.then((unlisten) => unlisten()); };
  }, []);

  const sendChat = (text: string) => {
    setMessages((current) => [...current, { from: "me", text, at: Date.now() }]);
    void api.sendChat(text).catch((error: unknown) => notify(friendlyError(error)));
  };

  const toggleChat = () => setChatOpen((open) => { if (!open) setChatUnread(0); return !open; });

  // The host signals it can deliver Ctrl+Alt+Del; hosts that can't never do.
  useEffect(() => {
    let stop: (() => void) | undefined;
    const listener = api.onSecureAttentionSupport(() => setCanSecureAttention(true));
    void listener.then((unlisten) => { stop = unlisten; });
    return () => { if (stop) stop(); else void listener.then((unlisten) => unlisten()); };
  }, []);

  const sendSecureAttention = () => void api.sendSecureAttention().catch((error: unknown) => notify(friendlyError(error)));

  // The host advertises it can restart; it signals when a restart begins.
  useEffect(() => {
    if (role !== "viewer") return;
    const listeners = Promise.all([
      api.onRestartSupport(() => setCanRestart(true)),
      api.onHostRestarting(() => setRestarting(true)),
    ]);
    return () => { void listeners.then((stops) => stops.forEach((stop) => stop())); };
  }, [role]);

  // Clear the restarting banner once the session is live again, and drop the
  // capability when the session ends.
  useEffect(() => {
    if (status === "in_session") setRestarting(false);
    else if (status !== "connecting") { setCanRestart(false); setRestarting(false); }
  }, [status]);

  // The host advertises privacy mode and reports its real state (the host
  // always restores it when the session ends).
  useEffect(() => {
    if (role !== "viewer") return;
    const listeners = Promise.all([
      api.onPrivacyModeSupport(() => setCanPrivacy(true)),
      api.onPrivacyModeChanged(({ on }) => setPrivacyOn(on)),
    ]);
    return () => { void listeners.then((stops) => stops.forEach((stop) => stop())); };
  }, [role]);

  useEffect(() => { if (status !== "in_session" && status !== "connecting") { setCanPrivacy(false); setPrivacyOn(false); } }, [status]);

  // The host offers TCP tunnels, and says when it refuses a connection.
  useEffect(() => {
    if (role !== "viewer") return;
    const listeners = Promise.all([
      api.onTunnelSupport(() => setCanTunnel(true)),
      api.onTunnelRefused(({ local_port, code }) => notify(`Tunnel on port ${local_port}: ${tunnelRefusal(code)}`)),
    ]);
    return () => { void listeners.then((stops) => stops.forEach((stop) => stop())); };
  }, [role, notify]);

  // Tunnels end with the connection; a reconnected host offers them again.
  // Ask too, in case the offer came before this screen was listening.
  useEffect(() => {
    if (role !== "viewer") return;
    if (status !== "in_session") { setCanTunnel(false); setTunnelsOpen(false); return; }
    let active = true;
    void api.getTunnelSupport().then((available) => { if (active && available) setCanTunnel(true); }).catch(() => {});
    return () => { active = false; };
  }, [role, status]);

  const togglePrivacy = (on: boolean) => {
    setPrivacyOn(on); // optimistic; the host's privacy_mode_changed confirms
    void api.setPrivacyMode(on).catch((error: unknown) => { setPrivacyOn(!on); notify(friendlyError(error)); });
  };

  const restartRemote = () => {
    if (!window.confirm("Restart the remote computer? It will come back only if it is set to start at sign-in. The session will reconnect automatically.")) return;
    setRestarting(true);
    void api.requestRemoteRestart().catch((error: unknown) => { setRestarting(false); notify(friendlyError(error)); });
  };

  useEffect(() => {
    if (role !== "viewer") return;
    let stop: (() => void) | undefined;
    // The host resends the list when its displays change and keeps
    // capturing the chosen one while it is still there.
    const listener = api.onMonitorList(({ monitors: next }) => {
      setMonitors(next);
      setSelectedMonitor((current) => next.some((monitor) => monitor.id === current) ? current : next.find((monitor) => monitor.is_primary)?.id ?? next[0]?.id);
    });
    void listener.then((unlisten) => { stop = unlisten; });
    return () => { if (stop) stop(); else void listener.then((unlisten) => unlisten()); };
  }, [role]);

  useEffect(() => {
    if (role !== "viewer") return;
    const listeners = Promise.all([
      api.onVirtualDisplaySupport(({ max }) => setVirtualMax(max)),
      // The host already switched to it; the list naming it came first.
      api.onVirtualDisplayAdded(({ id }) => setSelectedMonitor(id)),
      api.onVirtualDisplayRefused(({ code }) => notify(virtualDisplayRefusal(code))),
      api.onMultiMonitorSupport(({ max }) => setMultiMonitorMax(max)),
    ]);
    return () => { void listeners.then((stops) => stops.forEach((stop) => stop())); };
  }, [role, notify]);

  useEffect(() => {
    if (role !== "viewer") return;
    let stop: (() => void) | undefined;
    const listener = api.onLinkStats(setLink);
    void listener.then((unlisten) => { stop = unlisten; });
    return () => { if (stop) stop(); else void listener.then((unlisten) => unlisten()); };
  }, [role]);

  // A reconnect may land on the other path; show nothing until it reports.
  useEffect(() => { if (status !== "in_session") { setLink(undefined); setVirtualMax(undefined); setMultiMonitorMax(undefined); } }, [status]);

  const chooseMonitor = (id: number) => {
    setSelectedMonitor(id);
    void api.selectMonitor(id).catch((error: unknown) => notify(friendlyError(error)));
  };

  const addDisplay = () => {
    const { width, height, hidpi } = virtualDisplaySize(window.innerWidth, window.innerHeight, window.devicePixelRatio);
    void api.addVirtualDisplay(width, height, hidpi).catch((error: unknown) => notify(friendlyError(error)));
  };

  const removeDisplay = (id: number) => {
    void api.removeVirtualDisplay(id).catch((error: unknown) => notify(friendlyError(error)));
  };

  const canAddDisplay = virtualMax !== undefined && monitors.filter((monitor) => monitor.is_virtual).length < virtualMax;
  // The host can stream extra monitors in their own windows, and there is more
  // than one to spread out.
  const canOpenMonitorWindow = multiMonitorMax !== undefined && multiMonitorMax > 0 && monitors.length > 1;

  // A monitor window may have pointed input at its own monitor; take it back
  // for the main view when the pointer returns here.
  const reclaimInput = () => { if (selectedMonitor !== undefined) void api.setInputMonitor(selectedMonitor).catch(() => {}); };

  const openMonitorWindow = (id: number) => {
    const index = monitors.findIndex((monitor) => monitor.id === id);
    const name = monitorName(monitors[index]?.name ?? "", index < 0 ? 0 : index);
    void openInNewWindow(id, name).catch((error: unknown) => notify(friendlyError(error)));
  };

  const toggleAudioMuted = (muted: boolean) => {
    setAudioMuted(muted);
    void api.setAudioMuted(muted).catch((error: unknown) => { setAudioMuted(!muted); notify(friendlyError(error)); });
  };

  const stopRecording = () => {
    const recorder = recorderRef.current;
    if (!recorder || recorder.state === "inactive") return;
    recorder.stop();
    setRecordingActive(false);
    recordingStartedAtRef.current = undefined;
    recordingStreamRef.current?.getTracks().forEach((track) => track.stop());
    recordingStreamRef.current = undefined;
  };

  const toggleRecording = async () => {
    if (role !== "viewer") return;
    if (recordingActive) {
      stopRecording();
      return;
    }

    const canvas = canvasRef.current;
    if (!canvas?.captureStream || typeof MediaRecorder === "undefined") { notify("Recording isn't supported on this system."); return; }
    if (recordingStartingRef.current) return;
    recordingStartingRef.current = true;
    let id: number;
    try { id = await api.startRecording(`truedesk-session-${new Date().toISOString()}.webm`); }
    catch (error) { notify(`Could not start recording: ${friendlyError(error)}`); return; }
    finally { recordingStartingRef.current = false; }
    try {
      const stream = canvas.captureStream(60);
      const recorder = new MediaRecorder(stream, { mimeType: "video/webm" });
      recordingStreamRef.current = stream;
      recorderRef.current = recorder;
      // Chain writes so chunks land on disk in order; each is dropped from
      // memory once written.
      let writes = Promise.resolve();
      recorder.ondataavailable = (event) => {
        if (event.data.size === 0) return;
        writes = writes.then(() => event.data.arrayBuffer()).then((chunk) => api.appendRecording(id, chunk));
      };
      recorder.onstop = () => {
        recorderRef.current = undefined;
        void writes
          .then(
            () => api.finishRecording(id).then((path) => notify(`Recording saved to ${path}`, "info")),
            (error: unknown) => api.finishRecording(id).finally(() => notify(`Recording may be incomplete: ${friendlyError(error)}`)),
          )
          .catch((error: unknown) => notify(`Could not save recording: ${friendlyError(error)}`));
      };
      recordingStartedAtRef.current = Date.now();
      setRecordingElapsed(0);
      setRecordingActive(true);
      recorder.start(1000);
    } catch (error) {
      recordingStreamRef.current?.getTracks().forEach((track) => track.stop());
      recordingStreamRef.current = undefined;
      recorderRef.current = undefined;
      notify(`Could not start recording: ${friendlyError(error)}`);
      void api.finishRecording(id).catch(() => {});
    }
  };

  useEffect(() => {
    if (!recordingActive) return;
    const timer = window.setInterval(() => {
      if (recordingStartedAtRef.current) setRecordingElapsed(Math.floor((Date.now() - recordingStartedAtRef.current) / 1000));
    }, 1000);
    return () => window.clearInterval(timer);
  }, [recordingActive]);

  useEffect(() => () => stopRecording(), []);

  useEffect(() => {
    if (role !== "viewer") return;
    const listener = api.onAudioFrame((frame) => {
      const audio = audioRef.current ?? { context: new AudioContext(), nextTime: 0 };
      audioRef.current = audio;
      const context = audio.context;
      if (context.state === "suspended") void context.resume();
      const pcmBytes = b64ToBytes(frame.samples);
      const samples = new Int16Array(pcmBytes.buffer, pcmBytes.byteOffset, Math.floor(pcmBytes.byteLength / 2));
      const channels = Math.max(1, frame.channels);
      const buffer = context.createBuffer(channels, Math.floor(samples.length / channels), frame.sample_rate);
      for (let channel = 0; channel < channels; channel++) {
        const output = buffer.getChannelData(channel);
        for (let i = 0; i < output.length; i++) output[i] = samples[i * channels + channel] / 32768;
      }
      const source = context.createBufferSource();
      source.buffer = buffer;
      source.connect(context.destination);
      const start = Math.max(context.currentTime + 0.02, audio.nextTime);
      source.start(start);
      audio.nextTime = start + buffer.duration;
    });
    return () => { void listener.then((stop) => stop()); audioRef.current?.context.close(); audioRef.current = undefined; };
  }, [role]);

  useEffect(() => {
    if (role !== "viewer" || !canvasRef.current) return;

    const canvas = canvasRef.current;
    // Request failures (e.g. the control channel is still opening) are
    // harmless: the decoder asks again on the next dropped frame.
    const decoder = makeDecoder(canvas, { requestKeyframe: () => void api.requestKeyframe().catch(() => {}) });
    const stopVideo = api.startVideo(decoder.push);
    const detachInput = attachInput(canvas, (bytes) => viewOnlyRef.current ? Promise.resolve() : api.sendInput(bytes));

    return () => {
      stopVideo();
      detachInput();
      decoder.close();
    };
  }, [role]);

  return (
    <main className={`session-screen session-screen--${role}`}>
      {role === "viewer" && restarting && <div className="session-restarting" role="status"><span className="session-restarting__spinner" aria-hidden="true" />Remote is restarting. Reconnecting automatically…</div>}
      {role === "viewer" && <canvas ref={canvasRef} className="session-canvas" tabIndex={0} aria-label="Remote screen" onPointerEnter={reclaimInput} onPointerDown={reclaimInput} />}
      {role === "host" && <div className="session-host-overlay"><span className="eyebrow">TRUEDESK · SECURE SESSION</span><h1>You are being controlled by {remoteId}</h1><p>Your screen is shared for this session.</p></div>}
      {role === "host" && <HostControlPanel remoteId={remoteId} onDisconnect={() => void api.disconnectViewer().catch((error: unknown) => notify(friendlyError(error)))} />}
      <Toolbar remoteId={remoteId} role={role} status={status} link={link} monitors={monitors} selectedMonitor={selectedMonitor} onMonitorChange={chooseMonitor} canAddDisplay={canAddDisplay} onAddDisplay={addDisplay} onRemoveDisplay={removeDisplay} canOpenMonitorWindow={canOpenMonitorWindow} onOpenMonitorWindow={openMonitorWindow} clipboardSync={clipboardSync} onClipboardSyncChange={toggleClipboardSync} audioMuted={audioMuted} onAudioMutedChange={toggleAudioMuted} recordingActive={recordingActive} recordingElapsed={recordingElapsed} onRecordingToggle={() => void toggleRecording()} filesOpen={filesOpen} onFilesToggle={() => setFilesOpen((open) => !open)} chatOpen={chatOpen} onChatToggle={toggleChat} chatUnread={chatUnread} canTunnel={canTunnel} tunnelsOpen={tunnelsOpen} onTunnelsToggle={() => setTunnelsOpen((open) => !open)} viewOnly={viewOnly} onViewOnlyChange={setViewOnly} canSecureAttention={canSecureAttention} onSecureAttention={sendSecureAttention} canRestart={canRestart} onRestart={restartRemote} canPrivacy={canPrivacy} privacyOn={privacyOn} onPrivacyChange={togglePrivacy} onDisconnect={onDisconnect} />
      {role === "viewer" && <FileTransfer open={filesOpen} onOpenChange={onFilesOpenChange} />}
      {role === "viewer" && <ChatPanel open={chatOpen} messages={messages} onSend={sendChat} onClose={() => setChatOpen(false)} />}
      {role === "viewer" && canTunnel && <TunnelPanel open={tunnelsOpen} onClose={() => setTunnelsOpen(false)} />}
    </main>
  );
}
