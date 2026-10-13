/**
 * Session errors as sentences a user can act on. The entries for viewer
 * errors are the desktop app's (viewer/src/lib/notify.ts); the rest are
 * browser-only.
 */
const KNOWN_ERRORS: [RegExp, string][] = [
  [/target ID must be 9 digits|peer ID must be 9 digits/i, "Enter the 9-digit ID."],
  [/relay closed while connecting/i, "The relay closed the connection. Check the ID and try again."],
  [/check the password and connect again/i, "Wrong password. The host's password changes after every session, so check the current one and connect again."],
  [/could not reach the relay/i, "Couldn't reach the relay. Check your internet connection and try again."],
];

export function friendlyError(error: unknown): string {
  const raw = (error instanceof Error ? error.message : String(error)).trim();
  for (const [pattern, text] of KNOWN_ERRORS) {
    if (pattern.test(raw)) return text;
  }
  if (!raw) return "Something went wrong.";
  const sentence = raw[0].toUpperCase() + raw.slice(1);
  return /[.!?]$/.test(sentence) ? sentence : `${sentence}.`;
}

/** Why this browser can't run the viewer, or null when it can. */
export function unsupportedReason(scope: { VideoDecoder?: unknown; WebSocket?: unknown; isSecureContext?: boolean } = globalThis): string | null {
  if (scope.isSecureContext === false) return "Open this page over https:// to connect.";
  if (typeof scope.WebSocket === "undefined") return "This browser can't open WebSocket connections.";
  if (typeof scope.VideoDecoder === "undefined") {
    return "This browser can't decode the video stream (it lacks WebCodecs). Use a recent Chrome, Edge, Firefox or Safari.";
  }
  return null;
}
