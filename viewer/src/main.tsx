import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
// Fonts are bundled so the app makes no third-party requests.
import "@fontsource/inter/400.css";
import "@fontsource/inter/500.css";
import "@fontsource/inter/600.css";
import "@fontsource/jetbrains-mono/400.css";
import "@fontsource/jetbrains-mono/500.css";
import "@fontsource/jetbrains-mono/600.css";
import "@fontsource/space-grotesk/500.css";
import "@fontsource/space-grotesk/600.css";
import "@fontsource/space-grotesk/700.css";
import App from "./App";
import MonitorWindow from "./screens/MonitorWindow";

// A monitor window opens with ?monitor=<host monitor id>&label=<name>; the main
// app runs in every other window.
const params = new URLSearchParams(window.location.search);
const monitorParam = params.get("monitor");
const monitor = monitorParam === null ? null : Number(monitorParam);

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    {monitor !== null && Number.isFinite(monitor)
      ? <MonitorWindow monitor={monitor} label={params.get("label") ?? `Monitor ${monitor}`} />
      : <App />}
  </StrictMode>,
);
