import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type Status = "offline" | "online" | "connecting" | "in_session";
export interface Identity { id: string; password: string }
export interface AppConfig { relay_url: string; recent_ids: string[] }
export interface UnattendedStatus { enabled: boolean; has_password: boolean }
export interface PermissionStatus { screen_recording: boolean; accessibility: boolean }
export interface IncomingRequest { peer_id: string; fingerprint: string }
export interface SessionStarted { role: "host" | "viewer"; remote_id: string }
export interface SessionEnded { reason: string }
export interface ErrorEvent { message: string }
export interface StatusChanged { state: Status }
export interface VideoFrame {
  seq: number;
  timestamp_us: number;
  keyframe: boolean;
  width: number;
  height: number;
  data: number[];
}
export interface FileOffer { id: number; name: string; size: number; from: string }
export interface FileProgress { id: number; transferred: number; total: number; direction: "send" | "receive" | string }
export interface FileDone { id: number }
export interface FileError { id: number; message: string }

export const getIdentity = () => invoke<Identity>("get_identity");
export const rotatePassword = () => invoke<{ password: string }>("rotate_password");
export const getConfig = () => invoke<AppConfig>("get_config");
export const setRelay = (url: string) => invoke<void>("set_relay", { url });
export const getUnattendedStatus = () => invoke<UnattendedStatus>("get_unattended_status");
export const setUnattendedAccess = (enabled: boolean, password: string | null) => invoke<void>("set_unattended_access", { enabled, password });
export const permissionStatus = () => invoke<PermissionStatus>("permission_status");
export const openPermissionSettings = (which: "screen" | "accessibility") => invoke<void>("open_permission_settings", { which });
// Tauri v2 exposes Rust snake_case params to JS as camelCase (peerId/targetId).
export const respondToRequest = (peer_id: string, allow: boolean, always: boolean) => invoke<void>("respond_to_request", { peerId: peer_id, allow, always });
export const startHosting = () => invoke<void>("start_hosting");
export const stopHosting = () => invoke<void>("stop_hosting");
export const connect = (target_id: string, password: string) => invoke<void>("connect", { targetId: target_id, password });
export const disconnect = () => invoke<void>("disconnect");
export const sendInput = (bytes: Uint8Array) => invoke<void>("send_input", { bytes: Array.from(bytes) });
export const setClipboardSync = (enabled: boolean) => invoke<void>("set_clipboard_sync", { enabled });
export const sendFile = (path: string) => invoke<number>("send_file", { path });
export const respondFile = (id: number, accept: boolean) => invoke<void>("respond_file", { id, accept });

const on = <T>(event: string, handler: (payload: T) => void): Promise<UnlistenFn> => listen<T>(event, ({ payload }) => handler(payload));
export const onStatusChanged = (handler: (payload: StatusChanged) => void) => on("status_changed", handler);
export const onIncomingRequest = (handler: (payload: IncomingRequest) => void) => on("incoming_request", handler);
export const onSessionStarted = (handler: (payload: SessionStarted) => void) => on("session_started", handler);
export const onSessionEnded = (handler: (payload: SessionEnded) => void) => on("session_ended", handler);
export const onError = (handler: (payload: ErrorEvent) => void) => on("error", handler);
export const onVideoFrame = (handler: (frame: VideoFrame) => void) => on("video_frame", handler);
export const onFileOffer = (handler: (payload: FileOffer) => void) => on("file_offer", handler);
export const onFileProgress = (handler: (payload: FileProgress) => void) => on("file_progress", handler);
export const onFileDone = (handler: (payload: FileDone) => void) => on("file_done", handler);
export const onFileError = (handler: (payload: FileError) => void) => on("file_error", handler);
