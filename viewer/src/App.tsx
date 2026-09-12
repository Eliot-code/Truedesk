import { useEffect, useState } from "react";
import Home from "./screens/Home";
import * as api from "./lib/api";

type Screen = "home" | "session" | "permissions";

export default function App() {
  const [screen, setScreen] = useState<Screen>("home");
  const [status, setStatus] = useState<api.Status>("offline");
  const [request, setRequest] = useState<api.IncomingRequest | null>(null);
  useEffect(() => {
    let active = true;
    const unlisten = Promise.all([
      api.onStatusChanged(({ state }) => active && setStatus(state)),
      api.onIncomingRequest((payload) => active && setRequest(payload)),
      api.onSessionStarted(() => active && setScreen("session")),
      api.onSessionEnded(() => active && setScreen("home")),
    ]);
    return () => { active = false; void unlisten.then((fns) => fns.forEach((fn) => fn())); };
  }, []);
  if (screen === "home") return <Home status={status} incomingRequest={request} onDismissRequest={() => setRequest(null)} />;
  return <main className="placeholder-screen"><div className="placeholder-card"><span className="eyebrow">OPENDESK</span><h1>{screen === "session" ? "Session" : "Permissions"}</h1><p>This screen is ready for the next app-shell task.</p></div></main>;
}
