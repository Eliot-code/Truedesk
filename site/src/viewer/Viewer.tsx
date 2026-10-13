// The browser viewer itself lives in web/ (with its tests); this page only
// mounts it, the same way web/src/main.tsx does for a standalone build.
import { StrictMode } from "react";
import App from "../../../web/src/App";
import { loadCore } from "../../../web/src/lib/core";
import "../../../viewer/src/styles.css";
import "../../../web/src/styles.css";

export default function Viewer() {
  return (
    <StrictMode>
      <App loadCore={loadCore} />
    </StrictMode>
  );
}
