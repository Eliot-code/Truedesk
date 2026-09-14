import { useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import * as api from "../lib/api";
import { makeDecoder } from "../lib/decode";
import { attachInput } from "../lib/input";
import { attachRouter, openFrame, runKex, type FrameRouter } from "../lib/kex";
import { Tag, parseVideo, wrap } from "../lib/protocol";
import Toolbar from "../components/Toolbar";

export interface SessionProps {
  remoteId: string;
  role: "host" | "viewer";
  password?: string;
  onDisconnect: () => void;
}

/**
 * Starts the browser-side portion of a viewer session on a JS-owned transport.
 */
export async function runViewerFrameLoop(ws: WebSocket, canvas: HTMLCanvasElement, password: string): Promise<void> {
  const router: FrameRouter = attachRouter(ws);
  await router.waitPaired();
  await runKex(ws, router, password);
  const decode = makeDecoder(canvas);
  router.onVideo((sealed) => {
    void openFrame(sealed).then((plain) => {
      const frame = parseVideo(plain);
      canvas.width = frame.width;
      canvas.height = frame.height;
      decode(frame.data, frame.keyframe, frame.timestampUs);
    }).catch((error: unknown) => console.error("Failed to open video frame", error));
  });
  attachInput(ws, canvas, async (bytes) => {
    const sealed: number[] = await invoke("seal_input", { bytes: Array.from(bytes) });
    return new Uint8Array(sealed);
  });
}

export default function Session({ remoteId, role, password, onDisconnect }: SessionProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    if (role !== "viewer" || !canvasRef.current || !password) return;
    let active = true;
    let ws: WebSocket | null = null;
    const start = async () => {
      const { relay_url } = await api.getConfig();
      if (!active) return;
      ws = new WebSocket(relay_url);
      ws.binaryType = "arraybuffer";
      ws.addEventListener("open", () => {
        if (!ws || !active) return;
        const body = new TextEncoder().encode(JSON.stringify({ t: "connect", target: remoteId, ver: 1 }));
        ws.send(wrap(Tag.Control, body) as Uint8Array<ArrayBuffer>);
        void runViewerFrameLoop(ws, canvasRef.current!, password).catch((error: unknown) => {
          if (active) console.error("Viewer session failed", error);
        });
      }, { once: true });
      ws.addEventListener("close", () => { if (active) onDisconnect(); });
    };
    void start().catch((error: unknown) => { if (active) console.error("Could not start viewer session", error); });
    return () => {
      active = false;
      if (ws && ws.readyState !== WebSocket.CLOSED) ws.close();
    };
  }, [role, password, remoteId, onDisconnect]);

  const disconnect = () => {
    onDisconnect();
  };

  return (
    <main className={`session-screen session-screen--${role}`}>
      <canvas ref={canvasRef} className="session-canvas" tabIndex={0} aria-label="Remote screen" />
      {role === "host" && <div className="session-host-overlay"><span className="eyebrow">OPENDESK · SECURE SESSION</span><h1>You are being controlled by {remoteId}</h1><p>Your screen is shared for this session.</p></div>}
      <Toolbar remoteId={remoteId} onDisconnect={disconnect} />
    </main>
  );
}
