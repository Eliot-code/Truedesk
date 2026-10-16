import { useEffect, useRef, useState } from "react";
import * as api from "../lib/api";
import { makeDecoder } from "../lib/decode";

/**
 * A standalone window showing one of the host's monitors as its own live
 * stream (TeamViewer-style multi-monitor). It owns the aux stream: it asks the
 * host to open the stream on mount and to close it on unmount. View-only for
 * now; input stays on the main window.
 */
export default function MonitorWindow({ monitor, label }: { monitor: number; label: string }) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [problem, setProblem] = useState<string | null>(null);

  useEffect(() => {
    document.title = label;
  }, [label]);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;

    const decoder = makeDecoder(canvas, { requestKeyframe: () => {} });
    const stopVideo = api.startMonitorVideo(monitor, decoder.push);
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
      decoder.close();
      void api.closeMonitorStream(monitor).catch(() => {});
    };
  }, [monitor]);

  return (
    <main className="monitor-window">
      {problem && <p className="monitor-window__notice" role="alert">{problem}</p>}
      <canvas ref={canvasRef} className="monitor-window__canvas" aria-label={label} />
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
