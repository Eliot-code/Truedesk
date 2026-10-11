import { createContext, useContext } from "react";

export type NoticeTone = "error" | "info";
export interface Notice { id: number; text: string; tone: NoticeTone }
export type Notify = (text: string, tone?: NoticeTone) => void;

/** Shows a toast; the provider lives in App. Outside it, notices are dropped. */
export const NotifyContext = createContext<Notify>(() => {});
export const useNotify = () => useContext(NotifyContext);

/** Backend error strings mapped to sentences a user can act on. */
const KNOWN_ERRORS: [RegExp, string][] = [
  [/target ID must be 9 digits|peer ID must be 9 digits/i, "Enter the 9-digit ID."],
  [/viewer session is already active/i, "You are already connected to a device. Disconnect first."],
  [/viewer session was stopped before it started/i, "The connection was cancelled."],
  [/no active viewer session|viewer session is closed/i, "No device is connected."],
  [/control channel is not ready/i, "The session is still starting. Try again in a moment."],
  [/invalid 2FA code/i, "That code didn't match. Enter the current 6-digit code from your authenticator app."],
  [/no two-factor setup is in progress/i, "Two-factor setup expired. Turn it on again to start over."],
  [/two-factor is already enabled/i, "Two-factor is already on."],
  [/unattended access password must be at least (\d+)/i, "Use at least $1 characters for the unattended access password."],
  [/enter an unattended access password before enabling it/i, "Set a password before turning on unattended access."],
  [/at least one relay URL is required|relay URL cannot be empty/i, "Enter at least one relay URL."],
  [/relay closed while connecting/i, "The relay closed the connection. Check the ID and try again."],
  [/incoming request expired/i, "That request has expired."],
  [/incoming file offer expired|no incoming file offer with that id/i, "That file offer is no longer available."],
  [/check the password and connect again/i, "Wrong password. Check it and connect again."],
];

export function friendlyError(error: unknown): string {
  const raw = (error instanceof Error ? error.message : String(error)).trim();
  for (const [pattern, text] of KNOWN_ERRORS) {
    const match = pattern.exec(raw);
    if (match) return text.replace("$1", match[1] ?? "");
  }
  if (!raw) return "Something went wrong.";
  const sentence = raw[0].toUpperCase() + raw.slice(1);
  return /[.!?]$/.test(sentence) ? sentence : `${sentence}.`;
}
