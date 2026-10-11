import { useCallback, useEffect, useRef, useState } from "react";
import * as api from "../lib/api";
import { makeDecoder } from "../lib/decode";
import { attachInput } from "../lib/input";
import Toolbar from "../components/Toolbar";
import FileTransfer from "../components/FileTransfer";
import { friendlyError, useNotify } from "../lib/notify";

/** Decode a base64 IPC payload into bytes (frames arrive as base64, not number[]). */
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

export default function Session({ remoteId, role, status = "in_session", onDisconnect }: SessionProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const notify = useNotify();
  const [filesOpen, setFilesOpen] = useState(false);
  const onFilesOpenChange = useCallback((open: boolean) => setFilesOpen(open), []);
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
  const toggleClipboardSync = (enabled: boolean) => {
    setClipboardSync(enabled);
    void api.setClipboardSync(enabled).catch((error: unknown) => { setClipboardSync(!enabled); notify(friendlyError(error)); });
  };

  useEffect(() => {
    let mounted = true;
    void api.getConfig().then((config) => { if (mounted) setClipboardSync(config.clipboard_sync); }).catch(() => {});
    return () => { mounted = false; };
  }, []);

  useEffect(() => {
    if (role !== "viewer") return;
    let stop: (() => void) | undefined;
    const listener = api.onMonitorList(({ monitors: next }) => {
      setMonitors(next);
      setSelectedMonitor(next.find((monitor) => monitor.is_primary)?.id ?? next[0]?.id);
    });
    void listener.then((unlisten) => { stop = unlisten; });
    return () => { if (stop) stop(); else void listener.then((unlisten) => unlisten()); };
  }, [role]);

  const chooseMonitor = (id: number) => {
    setSelectedMonitor(id);
    void api.selectMonitor(id).catch((error: unknown) => notify(friendlyError(error)));
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
    try { id = await api.startRecording(`opendesk-session-${new Date().toISOString()}.webm`); }
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
    const { decode, close: closeDecoder } = makeDecoder(canvas);
    let active = true;
    let unlisten: (() => void) | undefined;

    const listener = api.onVideoFrame((frame) => {
      if (!active) return;
      canvas.width = frame.width;
      canvas.height = frame.height;
      decode(b64ToBytes(frame.data), frame.keyframe, frame.timestamp_us);
    });
    void listener.then((stop) => {
      if (active) unlisten = stop;
      else stop();
    });

    const detachInput = attachInput(canvas, api.sendInput);

    return () => {
      active = false;
      detachInput();
      closeDecoder();
      if (unlisten) unlisten();
      else void listener.then((stop) => stop());
    };
  }, [role]);

  return (
    <main className={`session-screen session-screen--${role}`}>
      {role === "viewer" && <canvas ref={canvasRef} className="session-canvas" tabIndex={0} aria-label="Remote screen" />}
      {role === "host" && <div className="session-host-overlay"><span className="eyebrow">OPENDESK · SECURE SESSION</span><h1>You are being controlled by {remoteId}</h1><p>Your screen is shared for this session.</p></div>}
      <Toolbar remoteId={remoteId} role={role} status={status} monitors={monitors} selectedMonitor={selectedMonitor} onMonitorChange={chooseMonitor} clipboardSync={clipboardSync} onClipboardSyncChange={toggleClipboardSync} audioMuted={audioMuted} onAudioMutedChange={toggleAudioMuted} recordingActive={recordingActive} recordingElapsed={recordingElapsed} onRecordingToggle={() => void toggleRecording()} filesOpen={filesOpen} onFilesToggle={() => setFilesOpen((open) => !open)} onDisconnect={onDisconnect} />
      {role === "viewer" && <FileTransfer open={filesOpen} onOpenChange={onFilesOpenChange} />}
    </main>
  );
}
