/**
 * The parts of the web-core wasm module the app uses, as interfaces so tests
 * can stand in for it. See crates/web-core/src/wasm.rs.
 */

export interface ViewerEvent {
  /** "send", "authenticated", "video", "monitors", "clipboard" or "ended". */
  readonly kind: string;
  /** Monitors JSON, clipboard text or the end message. */
  readonly text: string;
  takeBytes(): Uint8Array;
  free(): void;
}

export interface Viewer {
  receive(message: Uint8Array, nowMs: number): void;
  closed(): void;
  tick(nowMs: number): void;
  nextEvent(): ViewerEvent | undefined;
  input(event: Uint8Array): Uint8Array | undefined;
  clipboard(text: string): Uint8Array | undefined;
  requestKeyframe(): Uint8Array | undefined;
  selectMonitor(id: number): Uint8Array | undefined;
  bye(): Uint8Array | undefined;
  free(): void;
}

export interface Step {
  readonly retry: boolean;
  readonly delayMs: number;
  readonly message: string;
  free(): void;
}

export interface Core {
  createViewer(password: string, authCode: string): Viewer;
  connectFrame(target: string, viewerId: string): Uint8Array;
  afterFailure(message: string, sessionStarted: boolean, attempt: number, outageMs: number): Step;
}

export interface MonitorInfo {
  id: number;
  name: string;
  width: number;
  height: number;
  is_primary: boolean;
}
