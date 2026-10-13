import init, { afterFailure, connectFrame, Viewer } from "../wasm/web_core.js";
import type { Core } from "./types";

let ready: Promise<Core> | undefined;

/** Loads the wasm module once. */
export function loadCore(): Promise<Core> {
  ready ??= init().then(() => ({
    createViewer: (password: string, authCode: string) => new Viewer(password, authCode),
    connectFrame,
    afterFailure,
  }));
  return ready;
}
