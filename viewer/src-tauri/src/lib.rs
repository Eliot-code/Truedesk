mod config;
mod permissions;

use config::ConfigStore;
use futures_util::StreamExt;
use host_core::filexfer::{ConsentDecision, FileReceiver, FileSink, FileSource, FileSender};
use protocol::{crypto::{Opener, Sealer}, filexfer::FileMsg, frame, Tag};
use serde::Serialize;
use std::{collections::HashMap, path::{Path, PathBuf}, sync::{Arc, Mutex}};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{mpsc, oneshot};

struct RuntimeState {
    config: ConfigStore,
    password: Option<String>,
    host_thread: Option<HostThread>,
    viewer_task: Option<tauri::async_runtime::JoinHandle<()>>,
    viewer_input: Option<mpsc::Sender<Vec<u8>>>,
    pending_request: Option<oneshot::Sender<bool>>,
    pending_file_requests: HashMap<u64, oneshot::Sender<bool>>,
    file_outgoing: Option<mpsc::Sender<Vec<u8>>>,
    file_sealer: Option<Arc<Mutex<Sealer>>>,
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
#[derive(Serialize, Clone)]
#[allow(dead_code)]
pub(crate) struct FileOffer {
    id: u64,
    name: String,
    size: u64,
    from: String,
}
#[derive(Serialize, Clone)]
struct FileProgress {
    id: u64,
    transferred: u64,
    total: u64,
    direction: &'static str,
}
#[derive(Serialize, Clone)]
struct FileDone {
    id: u64,
}
#[derive(Serialize, Clone)]
struct FileError {
    id: u64,
    message: String,
}

/// A FileChannel sink for the reliable relay side-channel. The session loop
/// supplies the channel and session sealer when Task 5 keeps that relay alive.
struct ReliableFileSink {
    outgoing: mpsc::Sender<Vec<u8>>,
    sealer: Arc<Mutex<Sealer>>,
    offer_id: Option<oneshot::Sender<Result<u64, String>>>,
    transfer_id: Arc<Mutex<Option<u64>>>,
}

#[async_trait::async_trait]
impl FileSink for ReliableFileSink {
    async fn send(&mut self, message: FileMsg) {
        if let FileMsg::Offer { id, .. } = &message {
            *self.transfer_id.lock().unwrap() = Some(*id);
            if let Some(sender) = self.offer_id.take() {
                let _ = sender.send(Ok(*id));
            }
        }
        let plaintext = message.to_bytes();
        let sealed = self.sealer.lock().unwrap().seal(&plaintext);
        let _ = self.outgoing.send(frame::wrap(Tag::Data, &sealed)).await;
    }
}

impl ReliableFileSink {
    fn fail_before_offer(&mut self, error: &str) {
        if let Some(sender) = self.offer_id.take() {
            let _ = sender.send(Err(error.to_owned()));
        }
    }
}

#[allow(dead_code)]
pub(crate) struct ReliableFileSource {
    incoming: mpsc::Receiver<Vec<u8>>,
    opener: Arc<Mutex<Opener>>,
}

#[async_trait::async_trait]
impl FileSource for ReliableFileSource {
    async fn recv(&mut self) -> Option<FileMsg> {
        while let Some(message) = self.incoming.recv().await {
            let Some((Tag::Data, sealed)) = frame::unwrap(&message) else {
                continue;
            };
            let plaintext = self.opener.lock().unwrap().open(sealed)?;
            if let Some(message) = FileMsg::from_bytes(&plaintext) {
                return Some(message);
            }
        }
        None
    }
}

#[allow(dead_code)]
pub(crate) fn downloads_dir() -> PathBuf {
    if let Ok(path) = std::env::var("XDG_DOWNLOAD_DIR") {
        if !path.trim().is_empty() {
            return PathBuf::from(path);
        }
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    home.map(|path| path.join("Downloads"))
        .unwrap_or_else(|| PathBuf::from("Downloads"))
}

fn route_file_consent(
    pending: &mut HashMap<u64, oneshot::Sender<bool>>,
    id: u64,
    accept: bool,
) -> Result<(), String> {
    pending
        .remove(&id)
        .ok_or_else(|| "no incoming file offer with that id".to_owned())?
        .send(accept)
        .map_err(|_| "incoming file offer expired".to_owned())
}

/// Start the receiver half once the session loop has a relay Data channel.
/// Consent is awaited asynchronously before FileReceiver creates a temporary
/// file, so no Tokio worker is blocked by the UI response.
#[allow(dead_code)]
pub(crate) fn spawn_file_receiver(
    app: AppHandle,
    incoming: mpsc::Receiver<Vec<u8>>,
    opener: Arc<Mutex<Opener>>,
    from: String,
) {
    tauri::async_runtime::spawn(async move {
        let mut source = ReliableFileSource { incoming, opener };
        let pending_app = app.clone();
        let transfer_id = Arc::new(Mutex::new(None));
        let progress_id = transfer_id.clone();
        let offer_id = transfer_id.clone();
        let result = FileReceiver::receive_with_async_consent(
            &mut source,
            &downloads_dir(),
            move |offer| async move {
                let FileMsg::Offer { id, name, size, .. } = offer else {
                    return ConsentDecision::Reject;
                };
                *offer_id.lock().unwrap() = Some(id);
                let (decision_tx, decision_rx) = oneshot::channel();
                pending_app
                    .state::<AppState>()
                    .0
                    .lock()
                    .unwrap()
                    .pending_file_requests
                    .insert(id, decision_tx);
                emit(
                    &pending_app,
                    "file_offer",
                    FileOffer {
                        id,
                        name: name.clone(),
                        size,
                        from: from.clone(),
                    },
                );

                match decision_rx.await.unwrap_or(false) {
                    true => ConsentDecision::Accept,
                    false => ConsentDecision::Reject,
                }
            },
            {
                let progress_app = app.clone();
                move |transferred, total| {
                    if let Some(id) = *progress_id.lock().unwrap() {
                        emit(
                            &progress_app,
                            "file_progress",
                            FileProgress {
                                id,
                                transferred,
                                total,
                                direction: "receive",
                            },
                        );
                    }
                }
            },
        )
        .await;
        if let Err(error) = result {
            emit(
                &app,
                "file_error",
                FileError {
                    id: (*transfer_id.lock().unwrap()).unwrap_or(0),
                    message: error.to_string(),
                },
            );
        } else if let Some(id) = *transfer_id.lock().unwrap() {
            emit(&app, "file_done", FileDone { id });
        }
    });
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
async fn send_file(
    state: tauri::State<'_, AppState>,
    app: AppHandle,
    path: String,
) -> Result<u64, String> {
    let (outgoing, sealer) = {
        let state = state.0.lock().unwrap();
        (
            state.file_outgoing.clone(),
            state.file_sealer.clone(),
        )
    };
    let outgoing = outgoing.ok_or_else(|| "no active reliable file channel".to_owned())?;
    let sealer = sealer.ok_or_else(|| "file session keys are not ready".to_owned())?;
    let (offer_tx, offer_rx) = oneshot::channel();
    let task_app = app.clone();
    tauri::async_runtime::spawn(async move {
        let transfer_id = Arc::new(Mutex::new(None));
        let progress_id = transfer_id.clone();
        let progress_app = task_app.clone();
        let mut sink = ReliableFileSink {
            outgoing,
            sealer,
            offer_id: Some(offer_tx),
            transfer_id: transfer_id.clone(),
        };
        let result = FileSender::send(Path::new(&path), &mut sink, move |transferred, total| {
            if let Some(id) = *progress_id.lock().unwrap() {
                emit(
                    &progress_app,
                    "file_progress",
                    FileProgress {
                        id,
                        transferred,
                        total,
                        direction: "send",
                    },
                );
            }
        })
        .await;
        if let Err(error) = result {
            sink.fail_before_offer(&error.to_string());
            let id = *transfer_id.lock().unwrap();
            emit(
                &task_app,
                "file_error",
                FileError {
                    id: id.unwrap_or(0),
                    message: error.to_string(),
                },
            );
        } else if let Some(id) = *transfer_id.lock().unwrap() {
            emit(&task_app, "file_done", FileDone { id });
        }
    });
    // FileSender allocates the id when it emits Offer. Returning after that
    // first sealed frame lets the UI track a transfer while chunks continue.
    let id = offer_rx
        .await
        .map_err(|_| "file transfer stopped before sending its offer".to_owned())??;
    Ok(id)
}

#[tauri::command]
fn respond_file(
    state: tauri::State<'_, AppState>,
    id: u64,
    accept: bool,
) -> Result<(), String> {
    route_file_consent(
        &mut state.0.lock().unwrap().pending_file_requests,
        id,
        accept,
    )
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
                pending_file_requests: HashMap::new(),
                file_outgoing: None,
                file_sealer: None,
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
            send_input,
            send_file,
            respond_file
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

    #[test]
    fn file_consent_routes_by_transfer_id() {
        let (sender, mut receiver) = oneshot::channel();
        let mut pending = HashMap::new();
        pending.insert(42, sender);

        route_file_consent(&mut pending, 42, true).unwrap();

        assert!(receiver.try_recv().unwrap());
        assert!(route_file_consent(&mut pending, 42, false).is_err());
    }

    #[test]
    fn reliable_file_adapter_uses_sealed_data_frames() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (outgoing, mut incoming) = mpsc::channel(1);
            let key = [9u8; 32];
            let transfer_id = Arc::new(Mutex::new(None));
            let mut sink = ReliableFileSink {
                outgoing,
                sealer: Arc::new(Mutex::new(Sealer::new(key))),
                offer_id: None,
                transfer_id,
            };
            let message = FileMsg::Done { id: 7 };
            sink.send(message.clone()).await;
            let wire = incoming.recv().await.expect("data frame");
            let (tag, sealed) = frame::unwrap(&wire).expect("tagged frame");
            assert_eq!(tag, Tag::Data);
            let mut opener = Opener::new(key);
            assert_eq!(FileMsg::from_bytes(&opener.open(sealed).unwrap()), Some(message));
        });
    }
}
