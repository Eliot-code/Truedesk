import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { loadCore } from "./lib/core";
import "../../viewer/src/styles.css";
import "./styles.css";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App loadCore={loadCore} />
  </StrictMode>,
);
