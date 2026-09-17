mod config;
mod permissions;

use config::ConfigStore;
use futures_util::StreamExt;
use protocol::{frame, Tag};
use serde::Serialize;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{mpsc, oneshot};

struct RuntimeState {
    config: ConfigStore,
    password: Option<String>,
    host_thread: Option<HostThread>,
    viewer_task: Option<tauri::async_runtime::JoinHandle<()>>,
    viewer_input: Option<mpsc::Sender<Vec<u8>>>,
    pending_request: Option<oneshot::Sender<bool>>,
}

/// The host session owns Enigo, which is not `Send`. Keep the session and its
/// current-thread Tokio runtime together on a dedicated OS thread. The stop
/// signal is separate so Tauri commands can shut it down without aborting a
/// task that may be running non-`Send` code.
struct HostThread {
    stop_tx: oneshot::Sender<()>,
    join: std::thread::JoinHandle<()>,
}
struct AppState(Mutex<RuntimeState>);

#[derive(Serialize)]
struct Identity {
    id: String,
    password: String,
}
#[derive(Serialize)]
struct PasswordResponse {
    password: String,
}
#[derive(Serialize)]
struct AppConfig {
    relay_url: String,
    recent_ids: Vec<String>,
}
#[derive(Serialize)]
struct PermissionStatus {
    screen_recording: bool,
    accessibility: bool,
}
#[derive(Serialize, Clone)]
struct StatusEvent {
    state: &'static str,
}
#[derive(Serialize, Clone)]
struct IncomingRequest {
    peer_id: String,
    fingerprint: String,
}
#[derive(Serialize, Clone)]
struct SessionStarted {
    role: &'static str,
    remote_id: String,
}
#[derive(Serialize, Clone)]
struct SessionEnded {
    reason: String,
}
#[derive(Serialize, Clone)]
struct ErrorEvent {
    message: String,
}
#[derive(Serialize, Clone)]
struct VideoFrame {
    seq: u64,
    timestamp_us: u32,
    keyframe: bool,
    width: u16,
    height: u16,
    data: Vec<u8>,
}

fn emit<T: Serialize + Clone>(app: &AppHandle, event: &str, payload: T) {
    let _ = app.emit(event, payload);
}
fn fresh_password() -> String {
    use rand::Rng;
    rand::thread_rng().gen_range(100_000..=999_999).to_string()
}
fn current_password(state: &mut RuntimeState) -> String {
    state.password.get_or_insert_with(fresh_password).clone()
}

fn enqueue_input(sender: Option<mpsc::Sender<Vec<u8>>>, bytes: Vec<u8>) -> Result<(), String> {
    sender
        .ok_or_else(|| "no active viewer session".to_string())?
        .try_send(bytes)
        .map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => "viewer input queue is full".into(),
            mpsc::error::TrySendError::Closed(_) => "viewer session is closed".into(),
        })
}

#[tauri::command]
fn get_identity(state: tauri::State<'_, AppState>) -> Identity {
    let mut state = state.0.lock().unwrap();
    let id = state.config.ensure_identity();
    Identity {
        id,
        password: current_password(&mut state),
    }
}
#[tauri::command]
fn rotate_password(state: tauri::State<'_, AppState>) -> PasswordResponse {
    let mut state = state.0.lock().unwrap();
    let password = fresh_password();
    state.password = Some(password.clone());
    PasswordResponse { password }
}
#[tauri::command]
fn get_config(state: tauri::State<'_, AppState>) -> AppConfig {
    let state = state.0.lock().unwrap();
    AppConfig {
        relay_url: state.config.config.relay_url.clone(),
        recent_ids: state.config.config.recent_ids.clone(),
    }
}
#[tauri::command]
fn set_relay(state: tauri::State<'_, AppState>, url: String) -> Result<(), String> {
    if url.trim().is_empty() {
        return Err("relay URL cannot be empty".into());
    }
    let mut state = state.0.lock().unwrap();
    state.config.config.relay_url = url;
    state.config.save().map_err(|e| e.to_string())
}
#[tauri::command]
fn permission_status() -> PermissionStatus {
    PermissionStatus {
        screen_recording: permissions::screen_recording(),
        accessibility: permissions::accessibility(),
    }
}
#[tauri::command]
fn open_permission_settings(which: String) -> Result<(), String> {
    permissions::open_settings(&which)
}
#[tauri::command]
fn respond_to_request(
    state: tauri::State<'_, AppState>,
    peer_id: String,
    allow: bool,
    always: bool,
) -> Result<(), String> {
    let mut state = state.0.lock().unwrap();
    if always && allow && peer_id != "unknown" {
        state
            .config
            .add_allow(&peer_id)
            .map_err(|e| e.to_string())?;
    }
    state
        .pending_request
        .take()
        .ok_or_else(|| "no incoming request".to_string())?
        .send(allow)
        .map_err(|_| "incoming request expired".into())
}

#[tauri::command]
fn start_hosting(state: tauri::State<'_, AppState>, app: AppHandle) -> Result<(), String> {
    let (relay_url, id, password) = {
        let mut s = state.0.lock().unwrap();
        let id = s.config.ensure_identity();
        (
            s.config.config.relay_url.clone(),
            id,
            current_password(&mut s),
        )
    };
    if state.0.lock().unwrap().host_thread.is_some() {
        return Ok(());
    }
    emit(
        &app,
        "status_changed",
        StatusEvent {
            state: "connecting",
        },
    );
    let task = tauri::async_runtime::spawn(async move {
        let result = async {
            let mut ws = host_core::relay_client::connect(&relay_url)
                .await
                .map_err(|e| e.to_string())?;
            host_core::relay_client::register(&mut ws, &id)
                .await
                .map_err(|e| e.to_string())?;
            emit(&app, "status_changed", StatusEvent { state: "online" });
            let Some(Ok(tokio_tungstenite::tungstenite::Message::Binary(message))) =
                ws.next().await
            else {
                return Err::<(), String>("relay closed while hosting".into());
            };
            let Some((Tag::Control, body)) = frame::unwrap(&message) else {
                return Err("invalid pairing message".into());
            };
            let Some(protocol::control::Control::Paired { session, .. }) =
                protocol::control::Control::from_bytes(body)
            else {
                return Err("invalid pairing message".into());
            };
            let (decision_tx, decision_rx) = oneshot::channel();
            // Relay's current Paired frame does not carry the requester's ID.
            emit(
                &app,
                "incoming_request",
                IncomingRequest {
                    peer_id: "unknown".into(),
                    fingerprint: "pending".into(),
                },
            );
            app.state::<AppState>().0.lock().unwrap().pending_request = Some(decision_tx);
            if !decision_rx
                .await
                .map_err(|_| "incoming request expired".to_string())?
            {
                emit(
                    &app,
                    "session_ended",
                    SessionEnded {
                        reason: "rejected".into(),
                    },
                );
                emit(&app, "status_changed", StatusEvent { state: "offline" });
                return Ok(());
            }
            emit(
                &app,
                "status_changed",
                StatusEvent {
                    state: "in_session",
                },
            );
            emit(
                &app,
                "session_started",
                SessionStarted {
                    role: "host",
                    remote_id: "unknown".into(),
                },
            );
            let (stop_tx, stop_rx) = oneshot::channel::<()>();
            let host_app = app.clone();
            let join = std::thread::spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        emit(&host_app, "error", ErrorEvent { message: error.to_string() });
                        emit(
                            &host_app,
                            "session_ended",
                            SessionEnded { reason: "runtime initialization failed".into() },
                        );
                        emit(&host_app, "status_changed", StatusEvent { state: "offline" });
                        return;
                    }
                };

                let cleanup_app = host_app.clone();
                runtime.block_on(async move {
                    let result = tokio::select! {
                        result = host_core::roles::run_default_host_session(ws, &password, &session) => Some(result),
                        _ = stop_rx => None,
                    };
                    if let Some(Err(error)) = &result {
                        emit(&host_app, "error", ErrorEvent { message: error.to_string() });
                    }
                    emit(
                        &host_app,
                        "session_ended",
                        SessionEnded {
                            reason: if result.is_some() { "host session ended".into() } else { "disconnected".into() },
                        },
                    );
                    emit(&host_app, "status_changed", StatusEvent { state: "offline" });
                });

                // A naturally ended session should not leave a stale host
                // handle preventing a later start_hosting call.
                if let Ok(mut state) = cleanup_app.state::<AppState>().0.lock() {
                    state.host_thread = None;
                };
            });
            app.state::<AppState>().0.lock().unwrap().host_thread = Some(HostThread { stop_tx, join });
            Ok::<(), String>(())
        }
        .await;
        if let Err(message) = result {
            emit(&app, "error", ErrorEvent { message });
            emit(&app, "status_changed", StatusEvent { state: "offline" });
        }
    });
    // The registration task is intentionally short-lived after it hands the
    // paired socket to HostThread. Detach it (the host session is stopped via
    // the oneshot channel owned by HostThread).
    drop(task);
    Ok(())
}
#[tauri::command]
fn stop_hosting(state: tauri::State<'_, AppState>, app: AppHandle) {
    let (host_thread, pending_request) = {
        let mut s = state.0.lock().unwrap();
        (s.host_thread.take(), s.pending_request.take())
    };
    drop(pending_request);
    if let Some(host_thread) = host_thread {
        let _ = host_thread.stop_tx.send(());
        let _ = host_thread.join.join();
    } else {
        emit(&app, "status_changed", StatusEvent { state: "offline" });
    }
}

#[tauri::command]
fn connect(
    state: tauri::State<'_, AppState>,
    app: AppHandle,
    target_id: String,
    password: String,
) -> Result<(), String> {
    if target_id.len() != 9 || !target_id.bytes().all(|b| b.is_ascii_digit()) {
        return Err("target ID must be 9 digits".into());
    }
    state
        .0
        .lock()
        .unwrap()
        .config
        .add_recent(&target_id)
        .map_err(|e| e.to_string())?;
    emit(
        &app,
        "status_changed",
        StatusEvent {
            state: "connecting",
        },
    );
    let relay_url = state.0.lock().unwrap().config.config.relay_url.clone();
    let target = target_id.clone();
    let task_app = app.clone();
    let task = tauri::async_runtime::spawn(async move {
        let result = async {
            let mut ws = host_core::relay_client::connect(&relay_url)
                .await
                .map_err(|error| error.to_string())?;
            host_core::relay_client::connect_to(&mut ws, &target)
                .await
                .map_err(|error| error.to_string())?;

            let Some(Ok(tokio_tungstenite::tungstenite::Message::Binary(message))) =
                ws.next().await
            else {
                return Err::<(), String>("relay closed while connecting".into());
            };
            let Some((Tag::Control, body)) = frame::unwrap(&message) else {
                return Err("invalid pairing message".into());
            };
            let Some(protocol::control::Control::Paired { session, .. }) =
                protocol::control::Control::from_bytes(body)
            else {
                return Err("invalid pairing message".into());
            };

            let (frame_tx, mut frame_rx) = mpsc::channel::<Vec<u8>>(64);
            let (input_tx, input_rx) = mpsc::channel::<Vec<u8>>(256);
            task_app.state::<AppState>().0.lock().unwrap().viewer_input = Some(input_tx);

            let frame_app = task_app.clone();
            tauri::async_runtime::spawn(async move {
                while let Some(plaintext) = frame_rx.recv().await {
                    if plaintext.len() < 18 {
                        continue;
                    }
                    let seq = u64::from_be_bytes(plaintext[0..8].try_into().unwrap());
                    let timestamp_us = u32::from_be_bytes(plaintext[8..12].try_into().unwrap());
                    let _codec = plaintext[12];
                    let flags = plaintext[13];
                    let width = u16::from_be_bytes(plaintext[14..16].try_into().unwrap());
                    let height = u16::from_be_bytes(plaintext[16..18].try_into().unwrap());
                    emit(
                        &frame_app,
                        "video_frame",
                        VideoFrame {
                            seq,
                            timestamp_us,
                            keyframe: flags & 1 != 0,
                            width,
                            height,
                            data: plaintext[18..].to_vec(),
                        },
                    );
                }
            });

            emit(
                &task_app,
                "status_changed",
                StatusEvent {
                    state: "in_session",
                },
            );
            emit(
                &task_app,
                "session_started",
                SessionStarted {
                    role: "viewer",
                    remote_id: target.clone(),
                },
            );
            host_core::roles::run_viewer_session(ws, &password, &session, frame_tx, input_rx).await
        }
        .await;

        if let Err(message) = &result {
            emit(
                &task_app,
                "error",
                ErrorEvent {
                    message: message.clone(),
                },
            );
        }
        let app_state = task_app.state::<AppState>();
        let mut state = app_state.0.lock().unwrap();
        state.viewer_input = None;
        state.viewer_task = None;
        drop(state);
        emit(
            &task_app,
            "session_ended",
            SessionEnded {
                reason: if result.is_ok() {
                    "ended".into()
                } else {
                    "error".into()
                },
            },
        );
        emit(
            &task_app,
            "status_changed",
            StatusEvent { state: "offline" },
        );
    });
    state.0.lock().unwrap().viewer_task = Some(task);
    Ok(())
}
#[tauri::command]
fn send_input(state: tauri::State<'_, AppState>, bytes: Vec<u8>) -> Result<(), String> {
    let sender = state.0.lock().unwrap().viewer_input.clone();
    enqueue_input(sender, bytes)
}

#[tauri::command]
fn disconnect(state: tauri::State<'_, AppState>, app: AppHandle) {
    let (host_thread, viewer_task) = {
        let mut s = state.0.lock().unwrap();
        s.viewer_input = None;
        (s.host_thread.take(), s.viewer_task.take())
    };
    if let Some(viewer_task) = viewer_task {
        viewer_task.abort();
    }
    if let Some(host_thread) = host_thread {
        let _ = host_thread.stop_tx.send(());
        let _ = host_thread.join.join();
    }
    emit(
        &app,
        "session_ended",
        SessionEnded {
            reason: "disconnected".into(),
        },
    );
    emit(&app, "status_changed", StatusEvent { state: "offline" });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let app_data_dir = app
                .path()
                .app_data_dir()
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            let config = ConfigStore::load(app_data_dir)?;
            app.manage(AppState(Mutex::new(RuntimeState {
                config,
                password: None,
                host_thread: None,
                viewer_task: None,
                viewer_input: None,
                pending_request: None,
            })));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_identity,
            rotate_password,
            get_config,
            set_relay,
            permission_status,
            open_permission_settings,
            respond_to_request,
            start_hosting,
            stop_hosting,
            connect,
            disconnect,
            send_input
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_input_enqueues_bytes() {
        let (sender, mut receiver) = mpsc::channel(1);
        enqueue_input(Some(sender), vec![1, 2, 3]).unwrap();
        assert_eq!(receiver.try_recv().unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn send_input_requires_active_session() {
        assert_eq!(
            enqueue_input(None, vec![]),
            Err("no active viewer session".into())
        );
    }
}
