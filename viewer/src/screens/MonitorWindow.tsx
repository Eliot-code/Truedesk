import { useEffect, useRef, useState } from "react";
import * as api from "../lib/api";
import { makeDecoder } from "../lib/decode";
import { attachInput } from "../lib/input";

/**
 * A standalone window showing one of the host's monitors as its own live
 * stream (TeamViewer-style multi-monitor). It owns the aux stream: it asks the
 * host to open the stream on mount and to close it on unmount. It takes over
 * input while the pointer is in it, so its clicks and keys land on its own
 * monitor.
 */
export default function MonitorWindow({ monitor, label }: { monitor: number; label: string }) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [problem, setProblem] = useState<string | null>(null);

  useEffect(() => {
    document.title = label;
  }, [label]);

  // Claim the host's input for this monitor whenever the pointer or keyboard
  // focus enters, so input never lands on another window's monitor.
  const claimInput = () => void api.setInputMonitor(monitor).catch(() => {});

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;

    const decoder = makeDecoder(canvas, { requestKeyframe: () => {} });
    const stopVideo = api.startMonitorVideo(monitor, decoder.push);
    const detachInput = attachInput(canvas, (bytes) => api.sendInput(bytes));
    // Ignored by hosts that don't support aux streams; a refusal arrives as
    // monitor_stream_refused below.
    void api.openMonitorStream(monitor).catch(() => {});

    const refused = api.onMonitorStreamRefused(({ id, code }) => {
      if (id === monitor) setProblem(refusedMessage(code));
    });
    const opened = api.onMonitorStreamOpened(({ id }) => {
      if (id === monitor) setProblem(null);
    });

    return () => {
      void refused.then((off) => off());
      void opened.then((off) => off());
      stopVideo();
      detachInput();
      decoder.close();
      void api.closeMonitorStream(monitor).catch(() => {});
    };
  }, [monitor]);

  return (
    <main className="monitor-window">
      {problem && <p className="monitor-window__notice" role="alert">{problem}</p>}
      <canvas ref={canvasRef} className="monitor-window__canvas" aria-label={label} tabIndex={0} onPointerEnter={claimInput} onPointerDown={claimInput} onFocus={claimInput} />
    </main>
  );
}

function refusedMessage(code: string): string {
  switch (code) {
    case "limit":
      return "The host is already sharing the most monitors it allows at once. Close another monitor window and try again.";
    case "unknown_display":
      return "That monitor is no longer available on the host.";
    default:
      return "The host could not open this monitor in a separate window.";
  }
}
