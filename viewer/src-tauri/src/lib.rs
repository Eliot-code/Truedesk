mod config;
mod contacts;
mod permissions;
mod unattended;

use config::ConfigStore;
use contacts::{Contact, ContactStore};
use futures_util::StreamExt;
use host_core::{clipboard::{ArboardBackend, ClipboardSync}, filexfer::{ConsentDecision, FileReceiver, FileSink, FileSource, FileSender}, roles::DataHandle, transport::reliable::ReliableSender};
use protocol::{control::{Control, MonitorInfo}, crypto::{Opener, Sealer}, datamsg::DataMsg, filexfer::FileMsg, frame, Tag};
use serde::Serialize;
use std::{collections::HashMap, fs, path::{Path, PathBuf}, sync::{atomic::{AtomicBool, Ordering}, mpsc as std_mpsc, Arc, Mutex}, time::Duration};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{mpsc, oneshot};

struct RuntimeState {
    config: ConfigStore,
    contacts: ContactStore,
    password: Option<String>,
    host_thread: Option<HostThread>,
    viewer_task: Option<tauri::async_runtime::JoinHandle<()>>,
    viewer_input: Option<mpsc::Sender<Vec<u8>>>,
    pending_request: Option<oneshot::Sender<bool>>,
    pending_file_requests: HashMap<u64, oneshot::Sender<bool>>,
    file_outgoing: Option<ReliableSender>,
    file_sealer: Option<Arc<Mutex<Sealer>>>,
    control_sender: Option<ReliableSender>,
    control_sealer: Option<Arc<Mutex<Sealer>>>,
    clipboard_enabled: Arc<AtomicBool>,
    audio_muted: Arc<AtomicBool>,
    clipboard_thread: Option<ClipboardThread>,
}

/// The host session owns Enigo, which is not `Send`. Keep the session and its
/// current-thread Tokio runtime together on a dedicated OS thread. The stop
/// signal is separate so Tauri commands can shut it down without aborting a
/// task that may be running non-`Send` code.
struct HostThread {
    stop_tx: oneshot::Sender<()>,
    join: std::thread::JoinHandle<()>,
}
struct ClipboardThread {
    command_tx: std_mpsc::Sender<ClipboardCommand>,
    join: std::thread::JoinHandle<()>,
}
enum ClipboardCommand {
    Apply(String),
    Stop,
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
struct UnattendedStatus {
    enabled: bool,
    has_password: bool,
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
struct MonitorListEvent { monitors: Vec<MonitorInfo> }
#[derive(Serialize, Clone)]
struct AudioFrameEvent { sample_rate: u32, channels: u8, samples: Vec<i16> }
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

/// A FileSink backed by the session's reliable, sealed Data channel.
struct ReliableFileSink {
    outgoing: ReliableSender,
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
        let plaintext = DataMsg::FileTransfer(message).to_bytes();
        let sealed = self.sealer.lock().unwrap().seal(&plaintext);
        let _ = self.outgoing.send_frame(Tag::Data, &sealed).await;
    }
}

impl ReliableFileSink {
    fn fail_before_offer(&mut self, error: &str) {
        if let Some(sender) = self.offer_id.take() {
            let _ = sender.send(Err(error.to_owned()));
        }
    }
}

pub(crate) struct ReliableFileSource {
    incoming: mpsc::Receiver<FileMsg>,
}

#[async_trait::async_trait]
impl FileSource for ReliableFileSource {
    async fn recv(&mut self) -> Option<FileMsg> {
        self.incoming.recv().await
    }
}

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

fn recording_path(suggested_name: &str) -> PathBuf {
    downloads_dir().join(sanitize_recording_name(suggested_name))
}

fn sanitize_recording_name(suggested_name: &str) -> String {
    let name = Path::new(suggested_name)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .replace('\\', "_");
    let sanitized: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect();
    let sanitized = sanitized.trim_matches('.');
    let sanitized = if sanitized.is_empty() { "recording" } else { sanitized };
    if sanitized.to_ascii_lowercase().ends_with(".webm") {
        sanitized.to_owned()
    } else {
        format!("{sanitized}.webm")
    }
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
pub(crate) fn spawn_file_receiver(
    app: AppHandle,
    incoming: mpsc::Receiver<FileMsg>,
    from: String,
) {
    tauri::async_runtime::spawn(async move {
        let mut source = ReliableFileSource { incoming };
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

fn spawn_clipboard_thread(
    outgoing: ReliableSender,
    sealer: Arc<Mutex<Sealer>>,
    enabled: Arc<AtomicBool>,
) -> Option<(std_mpsc::Sender<ClipboardCommand>, std::thread::JoinHandle<()>)> {
    let backend = ArboardBackend::new()?;
    let (command_tx, command_rx) = std_mpsc::channel();
    let join = std::thread::spawn(move || {
        let mut sync = ClipboardSync::new(backend);
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build();
        let Ok(runtime) = runtime else { return };
        loop {
            match command_rx.recv_timeout(Duration::from_millis(500)) {
                Ok(ClipboardCommand::Apply(text)) => {
                    if enabled.load(Ordering::Relaxed) { sync.apply(&text); }
                }
                Ok(ClipboardCommand::Stop) | Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
                Err(std_mpsc::RecvTimeoutError::Timeout) => {
                    if !enabled.load(Ordering::Relaxed) { continue; }
                    let Some(message) = sync.poll_data() else { continue };
                    let sealed = sealer.lock().unwrap().seal(&message.to_bytes());
                    if runtime.block_on(outgoing.send_frame(Tag::Data, &sealed)).is_err() { break; }
                }
            }
        }
    });
    Some((command_tx, join))
}

fn dispatch_data(
    incoming: mpsc::Receiver<Vec<u8>>,
    opener: Arc<Mutex<Opener>>,
    clipboard_tx: Option<std_mpsc::Sender<ClipboardCommand>>,
) -> mpsc::Receiver<FileMsg> {
    let (file_tx, file_rx) = mpsc::channel(64);
    tauri::async_runtime::spawn(async move {
        let mut incoming = incoming;
        while let Some(message) = incoming.recv().await {
            let Some(plaintext) = opener.lock().unwrap().open(&message) else { continue };
            match DataMsg::from_bytes(&plaintext) {
                Some(DataMsg::FileTransfer(message)) => { if file_tx.send(message).await.is_err() { break; } }
                Some(DataMsg::Clipboard(message)) => {
                    if let Some(sender) = &clipboard_tx { let _ = sender.send(ClipboardCommand::Apply(message.text)); }
                }
                None => {}
            }
        }
        drop(clipboard_tx);
    });
    file_rx
}

fn emit<T: Serialize + Clone>(app: &AppHandle, event: &str, payload: T) {
    let _ = app.emit(event, payload);
}

fn activate_data_channel(app: &AppHandle, handle: DataHandle, from: String) {
    let DataHandle { sender, receiver, sealer, opener, control_sender, control_sealer } = handle;
    let enabled = app.state::<AppState>().0.lock().unwrap().clipboard_enabled.clone();
    let sealer = Arc::new(Mutex::new(sealer));
    let clipboard_thread = spawn_clipboard_thread(sender.clone(), sealer.clone(), enabled);
    let clipboard_tx = clipboard_thread.as_ref().map(|(tx, _)| tx.clone());
    let file_receiver = dispatch_data(receiver, Arc::new(Mutex::new(opener)), clipboard_tx);
    let state = app.state::<AppState>();
    let mut state = state.0.lock().unwrap();
    state.file_outgoing = Some(sender);
    state.file_sealer = Some(sealer);
    state.control_sender = Some(control_sender);
    state.control_sealer = Some(Arc::new(Mutex::new(control_sealer)));
    state.clipboard_thread = clipboard_thread.map(|(command_tx, join)| ClipboardThread { command_tx, join });
    drop(state);
    spawn_file_receiver(app.clone(), file_receiver, from);
}

fn clear_data_channel(app: &AppHandle) {
    let state = app.state::<AppState>();
    // Take the thread handle out under the lock, then release the global state
    // lock BEFORE joining. The clipboard thread may be mid `block_on(send_frame)`
    // when it observes Stop; joining while holding the AppState mutex would
    // freeze every other Tauri command behind a network-dependent join.
    let thread = {
        let mut state = state.0.lock().unwrap();
        let thread = state.clipboard_thread.take();
        state.file_outgoing = None;
        state.file_sealer = None;
        state.control_sender = None;
        state.control_sealer = None;
        state.clipboard_enabled.store(true, Ordering::Relaxed);
        state.pending_file_requests.clear();
        thread
    };
    if let Some(thread) = thread {
        let _ = thread.command_tx.send(ClipboardCommand::Stop);
        let _ = thread.join.join();
    }
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
fn list_contacts(state: tauri::State<'_, AppState>) -> Vec<Contact> {
    state.0.lock().unwrap().contacts.list()
}
#[tauri::command]
fn add_contact(
    state: tauri::State<'_, AppState>,
    alias: String,
    peer_id: String,
) -> Result<Contact, String> {
    state.0.lock().unwrap().contacts.add(alias, peer_id).map_err(|e| e.to_string())
}
#[tauri::command]
fn remove_contact(state: tauri::State<'_, AppState>, peer_id: String) -> Result<(), String> {
    state.0.lock().unwrap().contacts.remove(&peer_id).map_err(|e| e.to_string())
}
#[tauri::command]
fn touch_contact(state: tauri::State<'_, AppState>, peer_id: String) -> Result<(), String> {
    state.0.lock().unwrap().contacts.touch(&peer_id).map_err(|e| e.to_string())
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
fn get_unattended_status(state: tauri::State<'_, AppState>) -> UnattendedStatus {
    let state = state.0.lock().unwrap();
    UnattendedStatus {
        enabled: state.config.config.unattended_enabled,
        has_password: unattended::has_password(state.config.config.unattended_password_set),
    }
}
#[tauri::command]
fn set_unattended_access(
    state: tauri::State<'_, AppState>,
    enabled: bool,
    password: Option<String>,
) -> Result<(), String> {
    let mut state = state.0.lock().unwrap();
    let password = password.filter(|value| !value.is_empty());
    if let Some(password) = password {
        unattended::set_password(&password).map_err(|error| format!("could not store unattended access password: {error}"))?;
    } else {
        unattended::delete_password().map_err(|error| format!("could not clear unattended access password: {error}"))?;
    }
    let has_password = unattended::get_password().is_ok();
    if enabled && !has_password {
        return Err("enter an unattended access password before enabling it".into());
    }
    state.config.config.unattended_password_set = has_password;
    state.config.config.unattended_enabled = enabled;
    state.config.save().map_err(|error| error.to_string())
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
fn save_recording(bytes: Vec<u8>, suggested_name: String) -> Result<String, String> {
    let directory = downloads_dir();
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let path = recording_path(&suggested_name);
    fs::write(&path, bytes).map_err(|error| error.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

#[tauri::command]
fn start_hosting(state: tauri::State<'_, AppState>, app: AppHandle) -> Result<(), String> {
    let (relay_url, id, password, unattended_enabled) = {
        let mut s = state.0.lock().unwrap();
        let id = s.config.ensure_identity();
        let unattended_password = if s.config.config.unattended_enabled {
            unattended::get_password().ok()
        } else {
            None
        };
        // A keychain failure deliberately disables unattended approval for
        // this session and uses the normal manual-accept PAKE path.
        let unattended_enabled = unattended_password.is_some();
        let password = unattended_password.unwrap_or_else(|| current_password(&mut s));
        (
            s.config.config.relay_url.clone(),
            id,
            password,
            unattended_enabled,
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
            let (stop_tx, stop_rx) = oneshot::channel::<()>();
            let host_app = app.clone();
            let approve_app = app.clone();
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
                    let (data_ready_tx, data_ready_rx) = oneshot::channel();
                    let mut stop_rx = stop_rx;
                    let approve = move || async move {
                        if unattended_enabled {
                            emit(&approve_app, "status_changed", StatusEvent { state: "in_session" });
                            emit(&approve_app, "session_started", SessionStarted { role: "host", remote_id: "unknown".into() });
                            return true;
                        }
                        let (decision_tx, decision_rx) = oneshot::channel();
                        // Relay's current Paired frame does not carry the requester's ID.
                        emit(&approve_app, "incoming_request", IncomingRequest {
                            peer_id: "unknown".into(), fingerprint: "pending".into(),
                        });
                        approve_app.state::<AppState>().0.lock().unwrap().pending_request = Some(decision_tx);
                        let allowed = decision_rx.await.unwrap_or(false);
                        if allowed {
                            emit(&approve_app, "status_changed", StatusEvent { state: "in_session" });
                            emit(&approve_app, "session_started", SessionStarted { role: "host", remote_id: "unknown".into() });
                        }
                        allowed
                    };
                    let mut session = Box::pin(host_core::roles::run_default_host_session_with_data_and_accept(
                        ws, &password, &session, unattended_enabled, approve, data_ready_tx,
                    ));
                    let result = tokio::select! {
                        handle = data_ready_rx => {
                            match handle {
                                Ok(handle) => {
                                    activate_data_channel(&host_app, handle, "unknown".into());
                                    tokio::select! {
                                        result = &mut session => Some(result),
                                        _ = &mut stop_rx => None,
                                    }
                                }
                                Err(_) => Some(Err("session ended before the data channel was ready".into())),
                            }
                        }
                        result = &mut session => Some(result),
                        _ = &mut stop_rx => None,
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
                    clear_data_channel(&host_app);
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
    // A contact is only touched when it already exists; the regular recent-ID
    // history above continues to cover connections made without saving one.
    let touch_result = state.0.lock().unwrap().contacts.touch(&target_id);
    if let Err(error) = touch_result {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(error.to_string());
        }
    }
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
            let (monitor_tx, mut monitor_rx) = mpsc::channel::<Vec<u8>>(16);
            let (audio_tx, mut audio_rx) = mpsc::channel::<Vec<u8>>(16);
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

            let audio_app = task_app.clone();
            let audio_muted = task_app.state::<AppState>().0.lock().unwrap().audio_muted.clone();
            tauri::async_runtime::spawn(async move {
                while let Some(body) = audio_rx.recv().await {
                    if audio_muted.load(Ordering::Relaxed) { continue; }
                    if let Some(frame) = protocol::audio::AudioFrame::from_bytes(&body) {
                        emit(&audio_app, "audio_frame", AudioFrameEvent { sample_rate: frame.sample_rate, channels: frame.channels, samples: frame.samples });
                    }
                }
            });

            let monitor_app = task_app.clone();
            tauri::async_runtime::spawn(async move {
                while let Some(body) = monitor_rx.recv().await {
                    if let Some(Control::MonitorList { monitors }) = Control::from_bytes(&body) {
                        emit(&monitor_app, "monitor_list", MonitorListEvent { monitors });
                    }
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
            let (data_ready_tx, data_ready_rx) = oneshot::channel();
            let session_task = tauri::async_runtime::spawn(async move {
                host_core::roles::run_viewer_session_with_monitors(
                    ws,
                    &password,
                    &session,
                    frame_tx,
                    input_rx,
                    Some(data_ready_tx),
                    Some(monitor_tx),
                    Some(audio_tx),
                )
                .await
            });
            match data_ready_rx.await {
                Ok(handle) => {
                    activate_data_channel(&task_app, handle, target.clone());
                    match session_task.await {
                        Ok(result) => result,
                        Err(error) => Err(format!("viewer session task failed: {error}")),
                    }
                }
                Err(_) => match session_task.await {
                    Ok(result) => result,
                    Err(error) => Err(format!("viewer session task failed: {error}")),
                },
            }
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
        clear_data_channel(&task_app);
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
async fn select_monitor(state: tauri::State<'_, AppState>, id: u32) -> Result<(), String> {
    let (sender, sealer) = {
        let state = state.0.lock().unwrap();
        (state.control_sender.clone(), state.control_sealer.clone())
    };
    let sender = sender.ok_or_else(|| "no active viewer session".to_owned())?;
    let sealer = sealer.ok_or_else(|| "control channel is not ready".to_owned())?;
    let body = Control::SelectMonitor { id }.to_bytes();
    let sealed = sealer.lock().unwrap().seal(&body);
    sender.send_frame(Tag::Control, &sealed).await.map_err(|error| error.to_string())
}

#[tauri::command]
fn set_clipboard_sync(state: tauri::State<'_, AppState>, enabled: bool) {
    state.0.lock().unwrap().clipboard_enabled.store(enabled, Ordering::Relaxed);
}

#[tauri::command]
fn set_audio_muted(state: tauri::State<'_, AppState>, muted: bool) {
    state.0.lock().unwrap().audio_muted.store(muted, Ordering::Relaxed);
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
    clear_data_channel(&app);
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
            let config = ConfigStore::load(&app_data_dir)?;
            let contacts = ContactStore::load(&app_data_dir)?;
            app.manage(AppState(Mutex::new(RuntimeState {
                config,
                contacts,
                password: None,
                host_thread: None,
                viewer_task: None,
                viewer_input: None,
                pending_request: None,
                pending_file_requests: HashMap::new(),
                file_outgoing: None,
                file_sealer: None,
                control_sender: None,
                control_sealer: None,
                clipboard_enabled: Arc::new(AtomicBool::new(true)),
                audio_muted: Arc::new(AtomicBool::new(false)),
                clipboard_thread: None,
            })));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_identity,
            rotate_password,
            get_config,
            list_contacts,
            add_contact,
            remove_contact,
            touch_contact,
            set_relay,
            get_unattended_status,
            set_unattended_access,
            permission_status,
            open_permission_settings,
            respond_to_request,
            start_hosting,
            stop_hosting,
            connect,
            disconnect,
            send_input,
            select_monitor,
            set_clipboard_sync,
            set_audio_muted,
            send_file,
            respond_file,
            save_recording
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
    fn reliable_file_source_opens_sealed_data_bodies() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let message = FileMsg::Done { id: 7 };
            let (file_tx, file_rx) = mpsc::channel(1);
            file_tx.send(message.clone()).await.unwrap();
            let mut source = ReliableFileSource {
                incoming: file_rx,
            };
            assert_eq!(source.recv().await, Some(message));
        });
    }

    #[test]
    fn recording_name_is_sanitized_and_has_webm_extension() {
        assert_eq!(sanitize_recording_name("../remote:session?.webm"), "remote_session_.webm");
        assert_eq!(sanitize_recording_name("nested/name"), "name.webm");
        assert_eq!(sanitize_recording_name("..."), "recording.webm");
    }

    #[test]
    fn recording_path_stays_in_downloads_directory() {
        let path = recording_path("../../capture");
        assert_eq!(path.parent(), Some(downloads_dir().as_path()));
        assert_eq!(path.file_name().and_then(|name| name.to_str()), Some("capture.webm"));
    }
}
