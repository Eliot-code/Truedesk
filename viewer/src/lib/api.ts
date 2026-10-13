import { Channel, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

export type Status = "offline" | "online" | "connecting" | "in_session";
export interface Identity { id: string; password: string }
export interface AppConfig { relay_url: string; relay_urls: string[]; recent_ids: string[]; clipboard_sync: boolean }
export interface UnattendedStatus { enabled: boolean; has_password: boolean; twofa_enabled: boolean }
export interface PermissionStatus { screen_recording: boolean; accessibility: boolean }
export interface IncomingRequest { peer_id: string; fingerprint: string; known: boolean }
export interface SessionStarted { role: "host" | "viewer"; remote_id: string }
export interface SessionEnded { reason: string }
export interface ErrorEvent { message: string }
export interface StatusChanged { state: Status }
export interface AudioFrame { sample_rate: number; channels: number; samples: string } // base64 of little-endian i16 PCM
export interface FileOffer { id: number; name: string; size: number; from: string }
export interface FileProgress { id: number; transferred: number; total: number; direction: "send" | "receive" | string }
export interface FileDone { id: number; path?: string | null }
export interface FileError { id: number; message: string }
export interface Contact { id: string; alias: string; last_connected: number | null }
export interface TotpEnrollment { uri: string; secret: string; qr_width: number; qr_modules: boolean[] }
export interface MonitorInfo { id: number; name: string; width: number; height: number; is_primary: boolean }
/** Media path of a viewer session and its measured round trip; `rtt_ms` is null until there is a real sample. */
export interface LinkStats { path: "direct" | "relay"; rtt_ms: number | null }

export const getIdentity = () => invoke<Identity>("get_identity");
export const rotatePassword = () => invoke<{ password: string }>("rotate_password");
export const getConfig = () => invoke<AppConfig>("get_config");
export const listContacts = () => invoke<Contact[]>("list_contacts");
export const listAllowed = () => invoke<string[]>("list_allowed");
export const addAllowed = (peer_id: string) => invoke<void>("add_allowed", { peerId: peer_id });
export const removeAllowed = (peer_id: string) => invoke<void>("remove_allowed", { peerId: peer_id });
export const addContact = (alias: string, peer_id: string) => invoke<Contact>("add_contact", { alias, peerId: peer_id });
export const removeContact = (peer_id: string) => invoke<void>("remove_contact", { peerId: peer_id });
export const touchContact = (peer_id: string) => invoke<void>("touch_contact", { peerId: peer_id });
export const setRelay = (url: string) => invoke<void>("set_relay", { url });
export const setRelays = (urls: string[]) => invoke<void>("set_relays", { urls });
export const listRelays = () => invoke<string[]>("list_relays");
export const getUnattendedStatus = () => invoke<UnattendedStatus>("get_unattended_status");
export const setUnattendedAccess = (enabled: boolean, password: string | null) => invoke<void>("set_unattended_access", { enabled, password });
/** Enabling returns a pending enrollment; 2FA is active only after confirmUnattended2fa. */
export const setUnattended2fa = (enabled: boolean) => invoke<TotpEnrollment | null>("set_unattended_2fa", { enabled });
export const confirmUnattended2fa = (code: string) => invoke<void>("confirm_unattended_2fa", { code });
export const permissionStatus = () => invoke<PermissionStatus>("permission_status");
export const openPermissionSettings = (which: "screen" | "accessibility") => invoke<void>("open_permission_settings", { which });
// Tauri v2 exposes Rust snake_case params to JS as camelCase (peerId/targetId).
export const respondToRequest = (peer_id: string, allow: boolean, always: boolean) => invoke<void>("respond_to_request", { peerId: peer_id, allow, always });
export const startHosting = () => invoke<void>("start_hosting");
export const stopHosting = () => invoke<void>("stop_hosting");
export const connect = (target_id: string, password: string, auth_code?: string) => invoke<void>("connect", auth_code ? { targetId: target_id, password, authCode: auth_code } : { targetId: target_id, password });
export const sendAuthCode = (code: string) => invoke<void>("send_auth_code", { code });
export const disconnect = () => invoke<void>("disconnect");
export const sendInput = (bytes: Uint8Array) => invoke<void>("send_input", { bytes: Array.from(bytes) });
export const selectMonitor = (id: number) => invoke<void>("select_monitor", { id });
export const requestKeyframe = () => invoke<void>("request_keyframe");
/**
 * Stream video frame messages (raw bytes: header plus access unit, see
 * video.ts) to `handler` over a binary channel. Call the returned function to
 * stop; a later startVideo from a remounted screen is not affected by it.
 */
export function startVideo(handler: (message: ArrayBuffer) => void): () => void {
  const channel = new Channel<ArrayBuffer>(handler);
  const started = invoke<void>("start_video", { channel });
  let stopped = false;
  return () => {
    if (stopped) return;
    stopped = true;
    channel.onmessage = () => {};
    // After start resolves, so the backend never sees stop before start.
    void started.then(() => invoke<void>("stop_video", { id: channel.id })).catch(() => {});
  };
}
export const setClipboardSync = (enabled: boolean) => invoke<void>("set_clipboard_sync", { enabled });
export const setAudioMuted = (muted: boolean) => invoke<void>("set_audio_muted", { muted });
export const sendFile = (path: string) => invoke<number>("send_file", { path });
/** Native file picker; resolves to the chosen path or null when cancelled. */
export const pickFile = async (): Promise<string | null> => {
  const selected = await openDialog({ multiple: false, directory: false, title: "Send file" });
  return typeof selected === "string" ? selected : null;
};
/** Files dropped anywhere on the window; `over` reports drag hover state. */
export const onFileDrop = (handler: (event: { type: "over" | "drop" | "leave"; paths: string[] }) => void): Promise<UnlistenFn> =>
  getCurrentWebview().onDragDropEvent(({ payload }) => {
    if (payload.type === "drop") handler({ type: "drop", paths: payload.paths });
    else if (payload.type === "leave") handler({ type: "leave", paths: [] });
    else handler({ type: "over", paths: [] });
  });
export const respondFile = (id: number, accept: boolean) => invoke<void>("respond_file", { id, accept });
// Recordings stream to disk in chunks (base64, like audio events) so long
// sessions are never held in memory.
export const startRecording = (suggestedName: string) => invoke<number>("start_recording", { suggestedName });
export const appendRecording = (id: number, chunk: ArrayBuffer) => invoke<void>("append_recording", { id, chunk: toBase64(new Uint8Array(chunk)) });
export const finishRecording = (id: number) => invoke<string>("finish_recording", { id });

function toBase64(bytes: Uint8Array): string {
  let binary = "";
  for (let i = 0; i < bytes.length; i += 0x8000) binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(binary);
}

const on = <T>(event: string, handler: (payload: T) => void): Promise<UnlistenFn> => listen<T>(event, ({ payload }) => handler(payload));
export const onStatusChanged = (handler: (payload: StatusChanged) => void) => on("status_changed", handler);
export const onIncomingRequest = (handler: (payload: IncomingRequest) => void) => on("incoming_request", handler);
export const onSessionStarted = (handler: (payload: SessionStarted) => void) => on("session_started", handler);
export const onSessionEnded = (handler: (payload: SessionEnded) => void) => on("session_ended", handler);
export const onError = (handler: (payload: ErrorEvent) => void) => on("error", handler);
export const onMonitorList = (handler: (payload: { monitors: MonitorInfo[] }) => void) => on("monitor_list", handler);
export const onLinkStats = (handler: (payload: LinkStats) => void) => on("link_stats", handler);
export const onAudioFrame = (handler: (frame: AudioFrame) => void) => on("audio_frame", handler);
export const onFileOffer = (handler: (payload: FileOffer) => void) => on("file_offer", handler);
export const onFileProgress = (handler: (payload: FileProgress) => void) => on("file_progress", handler);
export const onFileDone = (handler: (payload: FileDone) => void) => on("file_done", handler);
export const onFileError = (handler: (payload: FileError) => void) => on("file_error", handler);
export const onPasswordRotated = (handler: (payload: { password: string }) => void) => on("password_rotated", handler);
