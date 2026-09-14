mod config;
mod permissions;

use config::ConfigStore;
use futures_util::StreamExt;
use protocol::crypto::{Opener, Sealer};
use protocol::{frame, Tag};
use serde::Serialize;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::oneshot;

/// Handshake and session state. Secrets and protocol keys never cross into JS.
#[derive(Default)]
struct Session {
    pake: Option<protocol::pake::PakeState>,
    shared: Option<[u8; 32]>,
    opener: Option<Opener>,
    sealer: Option<Sealer>,
}

struct RuntimeState {
    config: ConfigStore,
    password: Option<String>,
    session: Session,
    host_thread: Option<HostThread>,
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
            let Some(protocol::control::Control::Paired { .. }) =
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
                        result = host_core::roles::run_default_host_session(ws, &password) => Some(result),
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
    _password: String,
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
    emit(
        &app,
        "session_started",
        SessionStarted {
            role: "viewer",
            remote_id: target_id,
        },
    );
    Ok(())
}
#[tauri::command]
fn disconnect(state: tauri::State<'_, AppState>, app: AppHandle) {
    let host_thread = {
        let mut s = state.0.lock().unwrap();
        s.host_thread.take()
    };
    if let Some(host_thread) = host_thread {
        let _ = host_thread.stop_tx.send(());
        let _ = host_thread.join.join();
    } else {
        emit(
            &app,
            "session_ended",
            SessionEnded {
                reason: "disconnected".into(),
            },
        );
        emit(&app, "status_changed", StatusEvent { state: "offline" });
    }
}

#[tauri::command]
fn kex_start(state: tauri::State<'_, AppState>, password: String) -> Vec<u8> {
    let (pk, msg) = protocol::pake::start(password.as_bytes(), false);
    let mut s = state.0.lock().unwrap();
    s.session.pake = Some(pk);
    let mut body = Vec::with_capacity(1 + msg.len());
    body.push(0x01);
    body.extend_from_slice(&msg);
    body
}
#[tauri::command]
fn kex_finish(state: tauri::State<'_, AppState>, peer_msg: Vec<u8>) -> Result<Vec<u8>, String> {
    let mut s = state.0.lock().unwrap();
    let pk = s.session.pake.take().ok_or("kex not started")?;
    let shared = protocol::pake::finish(pk, &peer_msg).ok_or("bad spake2 message")?;
    let (send, recv) = protocol::kex::derive_keys(&shared, false);
    s.session.opener = Some(Opener::new(recv));
    s.session.sealer = Some(Sealer::new(send));
    s.session.shared = Some(shared);
    let confirm = protocol::pake::confirm_tag(&shared, false);
    let mut body = Vec::with_capacity(1 + confirm.len());
    body.push(0x02);
    body.extend_from_slice(&confirm);
    Ok(body)
}
#[tauri::command]
fn kex_verify(state: tauri::State<'_, AppState>, peer_confirm: Vec<u8>) -> Result<(), String> {
    let s = state.0.lock().unwrap();
    let shared = s.session.shared.ok_or("no shared key")?;
    if protocol::pake::verify_peer(&shared, true, &peer_confirm) {
        Ok(())
    } else {
        Err("key confirmation failed — wrong password or tampering".into())
    }
}
#[tauri::command]
fn open_frame(state: tauri::State<'_, AppState>, sealed: Vec<u8>) -> Result<Vec<u8>, String> {
    let mut s = state.0.lock().unwrap();
    s.session
        .opener
        .as_mut()
        .ok_or("no session key")?
        .open(&sealed)
        .ok_or_else(|| "AEAD open failed".into())
}
#[tauri::command]
fn seal_input(state: tauri::State<'_, AppState>, bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    let mut s = state.0.lock().unwrap();
    s.session
        .sealer
        .as_mut()
        .ok_or_else(|| "no session key".to_string())
        .map(|sealer| sealer.seal(&bytes))
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
                session: Session::default(),
                host_thread: None,
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
            kex_start,
            kex_finish,
            kex_verify,
            open_frame,
            seal_input
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
