/** Helpers for the browser viewer's file transfers. */

/**
 * A transfer id unique to this viewer: below 2^53 so it survives a round trip
 * through a JavaScript number, and odd so it never collides with the host's
 * own id stream (which starts from a random even-or-odd 48-bit base).
 */
export function nextFileId(): number {
  return Math.floor(Math.random() * 2 ** 50) * 2 + 1;
}

/** Offer `data` to the user as a download named `name`. */
export function downloadBytes(name: string, data: Uint8Array) {
  const blob = new Blob([data as BlobPart]);
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = name;
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
  // Give the download time to start before releasing the object URL.
  setTimeout(() => URL.revokeObjectURL(url), 10_000);
}

/** A human-readable byte size, e.g. "1.2 MB". */
export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(value < 10 ? 1 : 0)} ${units[unit]}`;
}
