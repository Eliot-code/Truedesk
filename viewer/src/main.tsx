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

createRoot(document.getElementById("root")!).render(
  <StrictMode><App /></StrictMode>,
);
