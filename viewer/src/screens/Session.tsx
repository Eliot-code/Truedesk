import { useEffect, useRef } from "react";
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
      <Toolbar remoteId={remoteId} onDisconnect={onDisconnect} />
      <FileTransfer />
    </main>
  );
}
