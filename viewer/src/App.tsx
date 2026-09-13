import { useEffect, useState } from "react";
import Home from "./screens/Home";
import IncomingRequest from "./screens/IncomingRequest";
import Permissions from "./screens/Permissions";
import Session from "./screens/Session";
import * as api from "./lib/api";

type Screen = "home" | "session" | "permissions";

export default function App() {
  const [screen, setScreen] = useState<Screen>("home");
  const [status, setStatus] = useState<api.Status>("offline");
  const [request, setRequest] = useState<api.IncomingRequest | null>(null);
  const [session, setSession] = useState<api.SessionStarted | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    const unlisten = Promise.all([
      api.onStatusChanged(({ state }) => active && setStatus(state)),
      api.onIncomingRequest((payload) => {
        if (!active) return;
        setRequest(payload);
        setError(null);
      }),
      api.onSessionStarted((payload) => {
        if (!active) return;
        setRequest(null);
        setSession(payload);
        setScreen("session");
        setError(null);
      }),
      api.onSessionEnded(() => {
        if (!active) return;
        setRequest(null);
        setSession(null);
        setScreen("home");
      }),
      api.onError(({ message }) => active && setError(message)),
    ]);

    return () => {
      active = false;
      void unlisten.then((fns) => fns.forEach((fn) => fn()));
    };
  }, []);

  let content;
  if (screen === "permissions") {
    content = <Permissions onContinue={() => setScreen("home")} />;
  } else if (screen === "session" && session) {
    content = <Session remoteId={session.remote_id} role={session.role} onDisconnect={() => void api.disconnect()} />;
  } else {
    content = <Home status={status} onOpenPermissions={() => setScreen("permissions")} />;
  }

  return <>
    {content}
    {error && <p className="message" role="alert">{error}</p>}
    {request && <IncomingRequest peerId={request.peer_id} fingerprint={request.fingerprint} onResolved={() => setRequest(null)} />}
  </>;
}
