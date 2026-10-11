//! Event payloads sent to the UI, and `emit` (which also logs lifecycle events).

use super::*;

#[derive(Serialize)]
pub(crate) struct Identity {
    pub(crate) id: String,
    pub(crate) password: String,
}
#[derive(Serialize, Clone)]
pub(crate) struct PasswordResponse {
    pub(crate) password: String,
}
#[derive(Serialize)]
pub(crate) struct AppConfig {
    pub(crate) relay_url: String,
    pub(crate) relay_urls: Vec<String>,
    pub(crate) recent_ids: Vec<String>,
    pub(crate) clipboard_sync: bool,
}
#[derive(Serialize)]
pub(crate) struct UnattendedStatus {
    pub(crate) enabled: bool,
    pub(crate) has_password: bool,
    pub(crate) twofa_enabled: bool,
}
#[derive(Serialize)]
pub(crate) struct PermissionStatus {
    pub(crate) screen_recording: bool,
    pub(crate) accessibility: bool,
}
#[derive(Serialize, Clone)]
pub(crate) struct StatusEvent {
    pub(crate) state: &'static str,
}
#[derive(Serialize, Clone)]
pub(crate) struct IncomingRequest {
    pub(crate) peer_id: String,
    pub(crate) fingerprint: String,
    /// The claimed peer ID is on the allow list. Informational only: the ID
    /// is not authenticated, so known devices still need approval.
    pub(crate) known: bool,
}
#[derive(Serialize, Clone)]
pub(crate) struct SessionStarted {
    pub(crate) role: &'static str,
    pub(crate) remote_id: String,
}
#[derive(Serialize, Clone)]
pub(crate) struct SessionEnded {
    pub(crate) reason: String,
}
/// A pending 2FA enrollment: shown to the user until they confirm a code.
#[derive(Serialize)]
pub(crate) struct TotpEnrollment {
    pub(crate) uri: String,
    pub(crate) secret: String,
    /// QR modules for `uri`, row-major, `qr_width` x `qr_width`; true = dark.
    pub(crate) qr_width: usize,
    pub(crate) qr_modules: Vec<bool>,
}
#[derive(Serialize, Clone)]
pub(crate) struct ErrorEvent {
    pub(crate) message: String,
}
#[derive(Serialize, Clone)]
pub(crate) struct VideoFrame {
    pub(crate) seq: u64,
    pub(crate) timestamp_us: u32,
    pub(crate) keyframe: bool,
    pub(crate) width: u16,
    pub(crate) height: u16,
    // Base64 of the encoded frame bytes. Emitting raw bytes as a JSON array
    // (`number[]`) over the Tauri IPC costs ~4x the size and heavy parse time
    // at video framerate; base64 keeps the event JSON but shrinks the hot path.
    pub(crate) data: String,
}

/// Base64-encode bytes for the IPC event payload (avoids JSON `number[]`).
pub(crate) fn b64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}
#[derive(Serialize, Clone)]
pub(crate) struct MonitorListEvent { pub(crate) monitors: Vec<MonitorInfo> }
#[derive(Serialize, Clone)]
pub(crate) struct AudioFrameEvent { pub(crate) sample_rate: u32, pub(crate) channels: u8, pub(crate) samples: String }
#[derive(Serialize, Clone)]
#[allow(dead_code)]
pub(crate) struct FileOffer {
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) size: u64,
    pub(crate) from: String,
}
#[derive(Serialize, Clone)]
pub(crate) struct FileProgress {
    pub(crate) id: u64,
    pub(crate) transferred: u64,
    pub(crate) total: u64,
    pub(crate) direction: &'static str,
}
#[derive(Serialize, Clone)]
pub(crate) struct FileDone {
    pub(crate) id: u64,
    /// Where a received file was saved; `None` for outgoing transfers.
    pub(crate) path: Option<String>,
}
#[derive(Serialize, Clone)]
pub(crate) struct FileError {
    pub(crate) id: u64,
    pub(crate) message: String,
}

/// Lifecycle events that also go to the log file, so a bug report can include
/// what happened. Media and progress events are too frequent to log.
pub(crate) const LOGGED_EVENTS: &[&str] = &["status_changed", "session_started", "session_ended", "incoming_request", "error", "file_error"];

pub(crate) fn emit<T: Serialize + Clone>(app: &AppHandle, event: &str, payload: T) {
    if LOGGED_EVENTS.contains(&event) {
        let details = serde_json::to_string(&payload).unwrap_or_default();
        if event.ends_with("error") { log::warn!("{event} {details}"); } else { log::info!("{event} {details}"); }
    }
    let _ = app.emit(event, payload);
}
