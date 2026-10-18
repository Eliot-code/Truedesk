/** Small per-browser settings. Storage can be missing (private windows), so every access is guarded. */

const VIEWER_ID_KEY = "truedesk.viewerId";
const RELAY_KEY = "truedesk.relay";

export const DEFAULT_RELAY: string = __DEFAULT_RELAY__;

function read(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function write(key: string, value: string | null) {
  try {
    if (value === null) localStorage.removeItem(key);
    else localStorage.setItem(key, value);
  } catch {
    // Not persisted; fine for this visit.
  }
}

/** A random 9-digit ID, like a desktop install's (100000000..=999999999). */
export function generateViewerId(random: (array: Uint32Array<ArrayBuffer>) => void = (array) => crypto.getRandomValues(array)): string {
  const range = 900_000_000;
  // Rejection sampling keeps the result uniform.
  const limit = Math.floor(2 ** 32 / range) * range;
  const value = new Uint32Array(1);
  do random(value);
  while (value[0] >= limit);
  return String(100_000_000 + (value[0] % range));
}

/** The ID this browser shows to the host. Kept so the host sees the same one each time. */
export function viewerId(): string {
  const saved = read(VIEWER_ID_KEY);
  if (saved && /^[1-9]\d{8}$/.test(saved)) return saved;
  const id = generateViewerId();
  write(VIEWER_ID_KEY, id);
  return id;
}

/** Digits only, so "123 456 789" and "123-456-789" both work. */
export function normalizeId(value: string): string {
  return value.replace(/[\s-]/g, "");
}

export function formatId(id: string): string {
  return /^\d{9}$/.test(id) ? `${id.slice(0, 3)} ${id.slice(3, 6)} ${id.slice(6)}` : id;
}

/**
 * wss:// anywhere. Plain ws:// only to this computer and only in `vite dev`:
 * the production page's CSP allows wss:// alone.
 */
export function relayUrlError(url: string, allowLocal: boolean = import.meta.env.DEV): string | null {
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return "Enter a relay URL like wss://relay.example.com/ws.";
  }
  if (parsed.protocol === "wss:") return null;
  const loopback = ["localhost", "127.0.0.1", "[::1]"].includes(parsed.hostname);
  if (parsed.protocol === "ws:" && loopback && allowLocal) return null;
  return "The relay URL must start with wss://.";
}

export function savedRelay(): string {
  return read(RELAY_KEY) ?? DEFAULT_RELAY;
}

export function saveRelay(url: string) {
  write(RELAY_KEY, url === DEFAULT_RELAY ? null : url);
}
