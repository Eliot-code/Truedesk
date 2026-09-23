import { useEffect, useRef, useState } from "react";
import * as api from "../lib/api";
import { makeDecoder } from "../lib/decode";
import { attachInput } from "../lib/input";
import Toolbar from "../components/Toolbar";
import FileTransfer from "../components/FileTransfer";

export interface SessionProps {
  remoteId: string;
  role: "host" | "viewer";
  password?: string;
  onDisconnect: () => void;
}

export default function Session({ remoteId, role, onDisconnect }: SessionProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [clipboardSync, setClipboardSync] = useState(true);
  const [audioMuted, setAudioMuted] = useState(false);
  const audioRef = useRef<{ context: AudioContext; nextTime: number } | undefined>(undefined);
  const [monitors, setMonitors] = useState<api.MonitorInfo[]>([]);
  const [selectedMonitor, setSelectedMonitor] = useState<number>();
  const toggleClipboardSync = (enabled: boolean) => {
    setClipboardSync(enabled);
    void api.setClipboardSync(enabled).catch(() => setClipboardSync(!enabled));
  };

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
    void api.selectMonitor(id).catch(() => {});
  };

  const toggleAudioMuted = (muted: boolean) => {
    setAudioMuted(muted);
    void api.setAudioMuted(muted).catch(() => setAudioMuted(!muted));
  };

  useEffect(() => {
    if (role !== "viewer") return;
    const listener = api.onAudioFrame((frame) => {
      const audio = audioRef.current ?? { context: new AudioContext(), nextTime: 0 };
      audioRef.current = audio;
      const context = audio.context;
      if (context.state === "suspended") void context.resume();
      const channels = Math.max(1, frame.channels);
      const buffer = context.createBuffer(channels, Math.floor(frame.samples.length / channels), frame.sample_rate);
      for (let channel = 0; channel < channels; channel++) {
        const output = buffer.getChannelData(channel);
        for (let i = 0; i < output.length; i++) output[i] = frame.samples[i * channels + channel] / 32768;
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
    const decode = makeDecoder(canvas);
    let active = true;
    let unlisten: (() => void) | undefined;

    const listener = api.onVideoFrame((frame) => {
      if (!active) return;
      canvas.width = frame.width;
      canvas.height = frame.height;
      decode(new Uint8Array(frame.data), frame.keyframe, frame.timestamp_us);
    });
    void listener.then((stop) => {
      if (active) unlisten = stop;
      else stop();
    });

    const detachInput = attachInput(canvas, api.sendInput);

    return () => {
      active = false;
      detachInput();
      if (unlisten) unlisten();
      else void listener.then((stop) => stop());
    };
  }, [role]);

  return (
    <main className={`session-screen session-screen--${role}`}>
      <canvas ref={canvasRef} className="session-canvas" tabIndex={0} aria-label="Remote screen" />
      {role === "host" && <div className="session-host-overlay"><span className="eyebrow">OPENDESK · SECURE SESSION</span><h1>You are being controlled by {remoteId}</h1><p>Your screen is shared for this session.</p></div>}
      <Toolbar remoteId={remoteId} monitors={monitors} selectedMonitor={selectedMonitor} onMonitorChange={chooseMonitor} clipboardSync={clipboardSync} onClipboardSyncChange={toggleClipboardSync} audioMuted={audioMuted} onAudioMutedChange={toggleAudioMuted} onDisconnect={onDisconnect} />
      <FileTransfer />
    </main>
  );
}
