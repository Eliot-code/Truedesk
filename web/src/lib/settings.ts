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

// --- Hosting this device (the Android app's "Share this phone" screen) ---

const HOST_ID_KEY = "truedesk.hostId";
const HOST_TOKEN_KEY = "truedesk.hostToken";
const HOST_PASSWORD_KEY = "truedesk.hostPassword";

/** The ID a viewer dials to control this device. Kept so it stays the same. */
export function hostId(): string {
  const saved = read(HOST_ID_KEY);
  if (saved && /^[1-9]\d{8}$/.test(saved)) return saved;
  return resetHostId();
}

/** Picks a fresh host ID, e.g. after the relay reports the ID is in use. */
export function resetHostId(): string {
  const id = generateViewerId();
  write(HOST_ID_KEY, id);
  return id;
}

/** A random token that binds the host ID to this device on the relay. */
export function hostToken(): string {
  const saved = read(HOST_TOKEN_KEY);
  if (saved && /^[0-9a-f]{64}$/.test(saved)) return saved;
  const bytes = new Uint8Array(32);
  try {
    crypto.getRandomValues(bytes);
  } catch {
    for (let i = 0; i < bytes.length; i += 1) bytes[i] = Math.floor(Math.random() * 256);
  }
  const token = Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
  write(HOST_TOKEN_KEY, token);
  return token;
}

/** The password a viewer must enter to control this device. */
export function hostPassword(): string {
  const saved = read(HOST_PASSWORD_KEY);
  if (saved) return saved;
  const password = generateHostPassword();
  write(HOST_PASSWORD_KEY, password);
  return password;
}

export function setHostPassword(password: string) {
  write(HOST_PASSWORD_KEY, password || null);
}

/** A short, readable password: no look-alike characters. */
export function generateHostPassword(random: (array: Uint8Array<ArrayBuffer>) => void = (array) => crypto.getRandomValues(array)): string {
  const alphabet = "abcdefghjkmnpqrstuvwxyz23456789";
  const bytes = new Uint8Array(8);
  try {
    random(bytes);
  } catch {
    for (let i = 0; i < bytes.length; i += 1) bytes[i] = Math.floor(Math.random() * 256);
  }
  return Array.from(bytes, (b) => alphabet[b % alphabet.length]).join("");
}

/**
 * Prefill from an invite link, e.g. `/connect/?id=123456789&relay=wss://…`.
 * Only a valid 9-digit ID and an acceptable relay are returned; the password
 * is never carried in a link.
 */
export function inviteParams(search: string = typeof location === "undefined" ? "" : location.search): { id: string | null; relay: string | null } {
  try {
    const params = new URLSearchParams(search);
    const id = normalizeId(params.get("id") ?? "");
    const relay = (params.get("relay") ?? "").trim();
    return {
      id: /^\d{9}$/.test(id) ? id : null,
      relay: relay && relayUrlError(relay) === null ? relay : null,
    };
  } catch {
    return { id: null, relay: null };
  }
}
