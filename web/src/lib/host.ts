/**
 * Bridge to the `truedesk-host` Tauri plugin: lets the Android app share this
 * device (screen + control) with a viewer. In a plain browser there is no
 * plugin, so {@link hostSupported} is false and the Share screen stays hidden.
 */

/** The events the hosting engine reports, as parsed from its JSON lines. */
export type HostEvent =
  | { t: "status"; s: "connecting" | "online" | "in_session" }
  | { t: "request"; peer: string }
  | { t: "session_started"; peer: string }
  | { t: "session_ended" }
  | { t: "identity_in_use" }
  | { t: "error"; message: string };

export interface StartResult {
  started: boolean;
  /** "", "accessibility", "projection_denied" or "invalid". */
  reason: string;
}

export interface HostState {
  accessibilityEnabled: boolean;
  hosting: boolean;
}

type Invoke = (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;

function invoker(): Invoke | null {
  const internals = (globalThis as { __TAURI_INTERNALS__?: { invoke?: Invoke } }).__TAURI_INTERNALS__;
  return internals && typeof internals.invoke === "function" ? internals.invoke.bind(internals) : null;
}

/** Whether this build runs inside a Tauri app that carries the plugin. */
export function hostSupported(): boolean {
  return invoker() !== null;
}

/** Whether to offer sharing this device: the plugin is present and it's Android. */
export function canShareDevice(userAgent: string = navigator.userAgent): boolean {
  return hostSupported() && /Android/i.test(userAgent);
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const invoke = invoker();
  if (!invoke) throw new Error("hosting is only available in the Truedesk app");
  return invoke(`plugin:truedesk-host|${command}`, args) as Promise<T>;
}

export interface StartParams {
  relays: string[];
  id: string;
  token: string;
  password: string;
  unattended?: boolean;
}

export const host = {
  state: () => call<HostState>("host_state"),
  openAccessibilitySettings: () => call<void>("open_accessibility_settings"),
  start: (params: StartParams) =>
    call<StartResult>("start_hosting", { args: { unattended: false, ...params } }),
  stop: () => call<void>("stop_hosting"),
  answer: (allow: boolean) => call<void>("answer_request", { args: { allow } }),
  poll: (timeoutMs: number) =>
    call<{ event?: string | null }>("poll_event", { args: { timeoutMs } }),
};

/** Parses one event line; returns null for anything unrecognized. */
export function parseHostEvent(line: string): HostEvent | null {
  let value: unknown;
  try {
    value = JSON.parse(line);
  } catch {
    return null;
  }
  if (!value || typeof value !== "object") return null;
  const event = value as Record<string, unknown>;
  switch (event.t) {
    case "status":
      if (event.s === "connecting" || event.s === "online" || event.s === "in_session") {
        return { t: "status", s: event.s };
      }
      return null;
    case "request":
      return typeof event.peer === "string" ? { t: "request", peer: event.peer } : null;
    case "session_started":
      return typeof event.peer === "string" ? { t: "session_started", peer: event.peer } : null;
    case "session_ended":
      return { t: "session_ended" };
    case "identity_in_use":
      return { t: "identity_in_use" };
    case "error":
      return { t: "error", message: typeof event.message === "string" ? event.message : "error" };
    default:
      return null;
  }
}

export type SharePhase = "idle" | "starting" | "connecting" | "online" | "in_session";

export interface ShareState {
  phase: SharePhase;
  /** The ID of a viewer waiting for allow/deny, if any. */
  request: string | null;
  /** The ID of the connected viewer, if any. */
  peer: string | null;
  error: string | null;
  /** Set when the relay reports this host ID is already taken. */
  identityInUse: boolean;
}

export const initialShareState: ShareState = {
  phase: "idle",
  request: null,
  peer: null,
  error: null,
  identityInUse: false,
};

/** Folds one host event into the Share screen's state. */
export function reduceHostEvent(state: ShareState, event: HostEvent): ShareState {
  switch (event.t) {
    case "status": {
      const phase: SharePhase =
        event.s === "in_session" ? "in_session" : event.s === "online" ? "online" : "connecting";
      // Leaving a session clears the connected peer.
      return { ...state, phase, peer: event.s === "in_session" ? state.peer : null };
    }
    case "request":
      return { ...state, request: event.peer };
    case "session_started":
      return { ...state, phase: "in_session", peer: event.peer, request: null };
    case "session_ended":
      return { ...state, phase: state.phase === "idle" ? "idle" : "online", peer: null, request: null };
    case "identity_in_use":
      return { ...state, identityInUse: true, error: "This device ID is already in use; pick a new one." };
    case "error":
      return { ...state, error: event.message };
    default:
      return state;
  }
}
