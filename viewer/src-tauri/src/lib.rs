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
use std::{collections::HashMap, fs, fs::OpenOptions, io, path::{Path, PathBuf}, sync::{atomic::{AtomicBool, Ordering}, mpsc as std_mpsc, Arc, Mutex}, time::{Duration, Instant}};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{mpsc, oneshot};

struct RuntimeState {
    config: ConfigStore,
    contacts: ContactStore,
    password: Option<String>,
    host_thread: Option<HostThread>,
    /// Set while a `start_hosting` registration task is connecting/waiting for
    /// a pairing, before its `HostThread` exists. Together with `host_thread`
    /// this makes the hosting slot reservation atomic under one lock.
    host_starting: Option<HostReservation>,
    viewer_task: Option<tauri::async_runtime::JoinHandle<()>>,
    viewer_stop: Option<Arc<AtomicBool>>,
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
/// task that may be running non-`Send` code. The thread is the hosting loop
/// (see `spawn_host_thread`): it re-registers after each session until the
/// stop signal fires or its sender is dropped.
struct HostThread {
    stop_tx: oneshot::Sender<()>,
    join: std::thread::JoinHandle<()>,
    /// Identifies which `start_hosting` call owns this thread so a finishing
    /// thread only clears its own handle, never a newer one.
    token: Arc<()>,
}

/// Marker for an in-flight `start_hosting` call. `token` is compared with
/// `Arc::ptr_eq` (mirroring the viewer slot's `viewer_stop` marker) so a stale
/// registration task cannot install or release a newer reservation.
struct HostReservation {
    token: Arc<()>,
    task: Option<tauri::async_runtime::JoinHandle<()>>,
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

impl AppState {
    fn lock(&self) -> std::sync::MutexGuard<'_, RuntimeState> {
        lock_unpoisoned(&self.0)
    }
}

/// Poison-tolerant lock: a panic while another holder had the mutex must not
/// cascade into every later command or background task.
fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

trait AbortableTask {
    fn abort(&self);
}

impl<T> AbortableTask for tauri::async_runtime::JoinHandle<T> {
    fn abort(&self) {
        self.abort();
    }
}

struct AbortOnDrop<T: AbortableTask>(Option<T>);

impl<T: AbortableTask> AbortOnDrop<T> {
    fn new(task: T) -> Self {
        Self(Some(task))
    }
}

impl<T: AbortableTask> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        if let Some(task) = self.0.take() {
            task.abort();
        }
    }
}

impl<T> AbortOnDrop<tauri::async_runtime::JoinHandle<T>> {
    async fn join(&mut self) -> Result<T, tauri::Error> {
        self.0.as_mut().expect("task already joined").await
    }
}

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
    relay_urls: Vec<String>,
    recent_ids: Vec<String>,
}
#[derive(Serialize)]
struct UnattendedStatus {
    enabled: bool,
    has_password: bool,
    twofa_enabled: bool,
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
    // Base64 of the encoded frame bytes. Emitting raw bytes as a JSON array
    // (`number[]`) over the Tauri IPC costs ~4x the size and heavy parse time
    // at video framerate; base64 keeps the event JSON but shrinks the hot path.
    data: String,
}

/// Base64-encode bytes for the IPC event payload (avoids JSON `number[]`).
fn b64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}
#[derive(Serialize, Clone)]
struct MonitorListEvent { monitors: Vec<MonitorInfo> }
#[derive(Serialize, Clone)]
struct AudioFrameEvent { sample_rate: u32, channels: u8, samples: String }
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
            *lock_unpoisoned(&self.transfer_id) = Some(*id);
            if let Some(sender) = self.offer_id.take() {
                let _ = sender.send(Ok(*id));
            }
        }
        let plaintext = DataMsg::FileTransfer(message).to_bytes();
        let sealed = lock_unpoisoned(&self.sealer).seal(&[Tag::Data as u8], &plaintext);
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

/// Maximum size accepted for a recording returned by the viewer UI.
const MAX_RECORDING_SIZE: usize = 2 * 1024 * 1024 * 1024;

#[allow(dead_code)]
fn recording_path(suggested_name: &str) -> PathBuf {
    downloads_dir().join(sanitize_recording_name(suggested_name))
}

fn validate_recording_size(bytes: &[u8]) -> Result<(), String> {
    validate_recording_size_len(bytes.len())
}

fn validate_recording_size_len(size: usize) -> Result<(), String> {
    if size > MAX_RECORDING_SIZE {
        return Err(format!(
            "recording exceeds the {} GiB maximum size",
            MAX_RECORDING_SIZE / (1024 * 1024 * 1024)
        ));
    }
    Ok(())
}

fn recording_name_with_suffix(name: &str, suffix: usize) -> String {
    if suffix == 1 {
        return name.to_owned();
    }
    let stem = name.strip_suffix(".webm").unwrap_or(name);
    format!("{stem} ({suffix}).webm")
}

fn next_recording_path(directory: &Path, suggested_name: &str) -> Result<PathBuf, String> {
    let sanitized_name = sanitize_recording_name(suggested_name);
    for suffix in 1.. {
        let path = directory.join(recording_name_with_suffix(&sanitized_name, suffix));
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!("refusing to write through symlink: {}", path.display()));
            }
            Ok(_) => continue,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(path),
            Err(error) => return Err(error.to_string()),
        }
    }
    unreachable!("recording filename suffix range is unbounded")
}

fn can_reserve_viewer_slot(viewer_task_reserved: bool, viewer_stop_reserved: bool) -> bool {
    !viewer_task_reserved && !viewer_stop_reserved
}

/// The hosting slot is free only when no host thread is running and no
/// `start_hosting` registration is in flight.
fn can_reserve_host_slot(host_thread_running: bool, host_starting: bool) -> bool {
    !host_thread_running && !host_starting
}

/// Whether `token` still owns the in-flight hosting reservation.
fn owns_host_reservation(reservation: Option<&HostReservation>, token: &Arc<()>) -> bool {
    reservation.is_some_and(|reservation| Arc::ptr_eq(&reservation.token, token))
}

/// Admit `decision_tx` as the pending pairing request. Only one request may
/// wait for a user decision: if a live one is already pending, the newcomer is
/// handed back so the caller can reject it (the first keeps its UI prompt). A
/// stale sender whose requester already went away does not block new requests.
fn admit_pending_request(
    slot: &mut Option<oneshot::Sender<bool>>,
    decision_tx: oneshot::Sender<bool>,
) -> Result<(), oneshot::Sender<bool>> {
    if slot.as_ref().is_some_and(|existing| !existing.is_closed()) {
        return Err(decision_tx);
    }
    *slot = Some(decision_tx);
    Ok(())
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
                *lock_unpoisoned(&offer_id) = Some(id);
                let (decision_tx, decision_rx) = oneshot::channel();
                pending_app
                    .state::<AppState>()
                    .lock()
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
                    if let Some(id) = *lock_unpoisoned(&progress_id) {
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
                    id: (*lock_unpoisoned(&transfer_id)).unwrap_or(0),
                    message: error.to_string(),
                },
            );
        } else if let Some(id) = *lock_unpoisoned(&transfer_id) {
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
                    let sealed = lock_unpoisoned(&sealer).seal(&[Tag::Data as u8], &message.to_bytes());
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
            let Some(plaintext) = lock_unpoisoned(&opener).open(&[Tag::Data as u8], &message) else { continue };
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
    let enabled = app.state::<AppState>().lock().clipboard_enabled.clone();
    let sealer = Arc::new(Mutex::new(sealer));
    let clipboard_thread = spawn_clipboard_thread(sender.clone(), sealer.clone(), enabled);
    let clipboard_tx = clipboard_thread.as_ref().map(|(tx, _)| tx.clone());
    let file_receiver = dispatch_data(receiver, Arc::new(Mutex::new(opener)), clipboard_tx);
    let state = app.state::<AppState>();
    let mut state = state.lock();
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
        let mut state = state.lock();
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
    let mut state = state.lock();
    let id = state.config.ensure_identity();
    Identity {
        id,
        password: current_password(&mut state),
    }
}
#[tauri::command]
fn rotate_password(state: tauri::State<'_, AppState>) -> PasswordResponse {
    let mut state = state.lock();
    let password = fresh_password();
    state.password = Some(password.clone());
    PasswordResponse { password }
}
#[tauri::command]
fn get_config(state: tauri::State<'_, AppState>) -> AppConfig {
    let state = state.lock();
    AppConfig {
        relay_url: state.config.config.relay_url.clone(),
        relay_urls: state.config.config.relay_urls.clone(),
        recent_ids: state.config.config.recent_ids.clone(),
    }
}
#[tauri::command]
fn list_contacts(state: tauri::State<'_, AppState>) -> Vec<Contact> {
    state.lock().contacts.list()
}
#[tauri::command]
fn list_allowed(state: tauri::State<'_, AppState>) -> Vec<String> {
    state.lock().config.config.allow_list.clone()
}
#[tauri::command]
fn add_allowed(state: tauri::State<'_, AppState>, peer_id: String) -> Result<(), String> {
    if peer_id.len() != 9 || !peer_id.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("peer ID must be 9 digits".into());
    }
    state.lock().config.add_allow(&peer_id).map_err(|error| error.to_string())
}
#[tauri::command]
fn remove_allowed(state: tauri::State<'_, AppState>, peer_id: String) -> Result<(), String> {
    state.lock().config.remove_allow(&peer_id).map_err(|error| error.to_string())
}
#[tauri::command]
fn add_contact(
    state: tauri::State<'_, AppState>,
    alias: String,
    peer_id: String,
) -> Result<Contact, String> {
    state.lock().contacts.add(alias, peer_id).map_err(|e| e.to_string())
}
#[tauri::command]
fn remove_contact(state: tauri::State<'_, AppState>, peer_id: String) -> Result<(), String> {
    state.lock().contacts.remove(&peer_id).map_err(|e| e.to_string())
}
#[tauri::command]
fn touch_contact(state: tauri::State<'_, AppState>, peer_id: String) -> Result<(), String> {
    state.lock().contacts.touch(&peer_id).map_err(|e| e.to_string())
}
#[tauri::command]
fn set_relay(state: tauri::State<'_, AppState>, url: String) -> Result<(), String> {
    if url.trim().is_empty() {
        return Err("relay URL cannot be empty".into());
    }
    let mut state = state.lock();
    state.config.config.relay_url = url;
    state.config.config.relay_urls = vec![state.config.config.relay_url.clone()];
    state.config.save().map_err(|e| e.to_string())
}
#[tauri::command]
fn set_relays(state: tauri::State<'_, AppState>, urls: Vec<String>) -> Result<(), String> {
    let urls: Vec<String> = urls.into_iter().map(|url| url.trim().to_owned()).filter(|url| !url.is_empty()).collect();
    if urls.is_empty() { return Err("at least one relay URL is required".into()); }
    state.lock().config.set_relays(urls).map_err(|e| e.to_string())
}
#[tauri::command]
fn list_relays(state: tauri::State<'_, AppState>) -> Vec<String> {
    state.lock().config.config.relay_urls.clone()
}
#[tauri::command]
fn get_unattended_status(state: tauri::State<'_, AppState>) -> UnattendedStatus {
    let state = state.lock();
    UnattendedStatus {
        enabled: state.config.config.unattended_enabled,
        has_password: unattended::has_password(state.config.config.unattended_password_set),
        twofa_enabled: state.config.config.unattended_twofa_enabled && unattended::get_totp_secret().is_ok(),
    }
}

fn random_base32_secret() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut bytes = [0u8; 20];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut bytes);
    let mut output = String::with_capacity(32);
    let mut buffer = 0u32;
    let mut bits = 0u8;
    for byte in bytes {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            output.push(ALPHABET[((buffer >> bits) & 31) as usize] as char);
            buffer &= (1 << bits) - 1;
        }
    }
    if bits > 0 { output.push(ALPHABET[((buffer << (5 - bits)) & 31) as usize] as char); }
    output
}

#[tauri::command]
fn set_unattended_2fa(state: tauri::State<'_, AppState>, enabled: bool) -> Result<Option<String>, String> {
    let mut state = state.lock();
    if enabled {
        let secret = random_base32_secret();
        unattended::set_totp_secret(&secret).map_err(|error| format!("could not store TOTP secret: {error}"))?;
        state.config.config.unattended_twofa_enabled = true;
        state.config.save().map_err(|error| error.to_string())?;
        Ok(Some(format!("otpauth://totp/OpenDesk?secret={secret}&issuer=OpenDesk")))
    } else {
        unattended::delete_totp_secret().map_err(|error| format!("could not clear TOTP secret: {error}"))?;
        state.config.config.unattended_twofa_enabled = false;
        state.config.save().map_err(|error| error.to_string())?;
        Ok(None)
    }
}
#[tauri::command]
fn set_unattended_access(
    state: tauri::State<'_, AppState>,
    enabled: bool,
    password: Option<String>,
) -> Result<(), String> {
    let mut state = state.lock();
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
    let mut state = state.lock();
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
        let state = state.lock();
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
            if let Some(id) = *lock_unpoisoned(&progress_id) {
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
            let id = *lock_unpoisoned(&transfer_id);
            emit(
                &task_app,
                "file_error",
                FileError {
                    id: id.unwrap_or(0),
                    message: error.to_string(),
                },
            );
        } else if let Some(id) = *lock_unpoisoned(&transfer_id) {
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
        &mut state.lock().pending_file_requests,
        id,
        accept,
    )
}

#[tauri::command]
fn save_recording(bytes: Vec<u8>, suggested_name: String) -> Result<String, String> {
    validate_recording_size(&bytes)?;
    let directory = downloads_dir();
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let directory_metadata = fs::symlink_metadata(&directory).map_err(|error| error.to_string())?;
    if directory_metadata.file_type().is_symlink() || !directory_metadata.is_dir() {
        return Err("recording Downloads path must be a regular directory".into());
    }
    let directory = fs::canonicalize(&directory).map_err(|error| error.to_string())?;
    let mut path = next_recording_path(&directory, &suggested_name)?;
    let mut file = loop {
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => break file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                // A competing writer won the name race; choose another name
                // and retain create-new semantics so no file is clobbered.
                path = next_recording_path(&directory, &suggested_name)?;
            }
            Err(error) => return Err(error.to_string()),
        }
    };
    io::Write::write_all(&mut file, &bytes).map_err(|error| error.to_string())?;
    let metadata = file
        .metadata()
        .map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("recording target is not a regular file".into());
    }
    // The file was opened with `create_new`, which never follows an existing
    // symlink or overwrites an existing file.
    Ok(path.to_string_lossy().into_owned())
}

/// Per-session host credentials and policy. Read fresh for every pairing so a
/// rotated password or a changed unattended/2FA setting applies to the next
/// session of a long-lived hosting loop instead of the value from when
/// hosting started.
struct HostSessionParams {
    password: String,
    unattended_enabled: bool,
    twofa_enabled: bool,
    totp_secret: Option<String>,
}

fn host_session_params(s: &mut RuntimeState) -> HostSessionParams {
    let unattended_password = if s.config.config.unattended_enabled {
        unattended::get_password().ok()
    } else {
        None
    };
    // A keychain failure deliberately disables unattended approval for
    // this session and uses the normal manual-accept PAKE path.
    let unattended_enabled = unattended_password.is_some();
    let twofa_enabled = unattended_enabled && s.config.config.unattended_twofa_enabled;
    let totp_secret = if twofa_enabled { unattended::get_totp_secret().ok() } else { None };
    let password = unattended_password.unwrap_or_else(|| current_password(s));
    HostSessionParams { password, unattended_enabled, twofa_enabled, totp_secret }
}

/// The relay identity a hosting loop registers under. It is stable for the
/// loop's lifetime: re-registering with the same token is accepted by the
/// relay's ownership lease, so a host can come back after every session.
#[derive(Clone)]
struct HostRegistration {
    id: String,
    token: String,
}

/// Connect to the relay, register, and wait for a viewer to pair, retrying
/// with capped exponential backoff until a pairing arrives. Returns `None`
/// once `still_hosting` reports that this loop no longer owns the hosting
/// slot. Callers may also cancel by dropping the future (task abort or a
/// `select!` on the host thread's stop signal).
///
/// Status: "online" when a registration is parked on the relay; "connecting"
/// (plus one error event per failure streak) while registration is failing.
/// A healthy registration that the relay expires (its pairing timeout) is
/// renewed silently so the UI stays "online".
async fn register_host_until_paired(
    app: &AppHandle,
    registration: &HostRegistration,
    still_hosting: impl Fn() -> bool,
) -> Option<(host_core::relay_client::RelayWebSocket, host_core::relay_client::Pairing)> {
    let mut backoff = host_core::reconnect::HostRegistrationBackoff::new();
    let mut online = false;
    loop {
        let delay = backoff.delay();
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        if !still_hosting() {
            return None;
        }
        // Re-read per attempt so a relay change applies to the next registration.
        let relay_url = app.state::<AppState>().lock().config.config.relay_url.clone();
        let failure = match host_core::relay_client::register_host(&[relay_url], &registration.id, &registration.token).await {
            Err(error) => {
                backoff.record_failure();
                error
            }
            Ok(mut ws) => {
                if !online {
                    emit(app, "status_changed", StatusEvent { state: "online" });
                    online = true;
                }
                let parked_at = Instant::now();
                match host_core::relay_client::await_pairing(&mut ws).await {
                    Ok(pairing) => return Some((ws, pairing)),
                    Err(error) => {
                        backoff.record_parked_then_closed(parked_at.elapsed());
                        error
                    }
                }
            }
        };
        if backoff.consecutive_failures() == 0 {
            // A long-parked registration expired normally; renew it quietly.
            continue;
        }
        if online || backoff.consecutive_failures() == 1 {
            emit(app, "error", ErrorEvent { message: format!("{failure}; retrying host registration") });
            emit(app, "status_changed", StatusEvent { state: "connecting" });
        }
        online = false;
    }
}

/// Run one paired host session. Returns `true` when the stop signal ended it
/// (stop_hosting/disconnect), `false` when the session ended on its own.
async fn run_paired_host_session(
    app: &AppHandle,
    ws: host_core::relay_client::RelayWebSocket,
    pairing: host_core::relay_client::Pairing,
    params: HostSessionParams,
    stop_rx: &mut oneshot::Receiver<()>,
) -> bool {
    let host_core::relay_client::Pairing { session: ice_session, peer_id } = pairing;
    let HostSessionParams { password, unattended_enabled, twofa_enabled, totp_secret } = params;
    let allowlisted = host_core::roles::should_auto_accept_allowlisted(
        app.state::<AppState>().lock().config.is_allowed(&peer_id),
    );
    let remote_id = peer_id.clone();
    let approve_app = app.clone();
    let (data_ready_tx, data_ready_rx) = oneshot::channel();
    let approve = move || async move {
        if allowlisted {
            emit(&approve_app, "status_changed", StatusEvent { state: "in_session" });
            emit(&approve_app, "session_started", SessionStarted { role: "host", remote_id: peer_id.clone() });
            return true;
        }
        if unattended_enabled {
            emit(&approve_app, "status_changed", StatusEvent { state: "in_session" });
            emit(&approve_app, "session_started", SessionStarted { role: "host", remote_id: peer_id.clone() });
            return true;
        }
        let (decision_tx, decision_rx) = oneshot::channel();
        // Install before emitting so the UI can never answer a
        // prompt whose sender is not yet in place.
        let admitted = admit_pending_request(
            &mut approve_app.state::<AppState>().lock().pending_request,
            decision_tx,
        );
        if let Err(rejected_tx) = admitted {
            // Another request is already awaiting a decision:
            // reject this one and leave the existing prompt.
            let _ = rejected_tx.send(false);
            return false;
        }
        emit(&approve_app, "incoming_request", IncomingRequest {
            peer_id: peer_id.clone(), fingerprint: "pending".into(),
        });
        let allowed = decision_rx.await.unwrap_or(false);
        if allowed {
            emit(&approve_app, "status_changed", StatusEvent { state: "in_session" });
            emit(&approve_app, "session_started", SessionStarted { role: "host", remote_id: peer_id.clone() });
        }
        allowed
    };
    let mut session = Box::pin(host_core::roles::run_default_host_session_with_data_and_accept_and_2fa(
        ws, &password, &ice_session, unattended_enabled, twofa_enabled, totp_secret.as_deref(), approve, data_ready_tx,
    ));
    let result = tokio::select! {
        handle = data_ready_rx => {
            match handle {
                Ok(handle) => {
                    activate_data_channel(app, handle, remote_id.clone());
                    tokio::select! {
                        result = &mut session => Some(result),
                        _ = &mut *stop_rx => None,
                    }
                }
                Err(_) => Some(Err("session ended before the data channel was ready".into())),
            }
        }
        result = &mut session => Some(result),
        _ = &mut *stop_rx => None,
    };
    if let Some(Err(error)) = &result {
        emit(app, "error", ErrorEvent { message: error.to_string() });
    }
    emit(
        app,
        "session_ended",
        SessionEnded {
            reason: if result.is_some() { "host session ended".into() } else { "disconnected".into() },
        },
    );
    clear_data_channel(app);
    result.is_none()
}

/// The host session owns Enigo (not `Send`), so each session runs on this
/// dedicated thread's current-thread runtime. The thread is the hosting loop:
/// after a session ends on its own it re-registers with the relay under the
/// same ID/token and waits for the next pairing, so a dropped viewer can
/// reconnect. Only the stop signal (stop_hosting/disconnect) or losing
/// ownership of `host_thread` ends the loop.
fn spawn_host_thread(
    app: AppHandle,
    registration: HostRegistration,
    first: (host_core::relay_client::RelayWebSocket, host_core::relay_client::Pairing),
    mut stop_rx: oneshot::Receiver<()>,
    token: Arc<()>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(runtime) => runtime,
            Err(error) => {
                emit(&app, "error", ErrorEvent { message: error.to_string() });
                emit(&app, "session_ended", SessionEnded { reason: "runtime initialization failed".into() });
                emit(&app, "status_changed", StatusEvent { state: "offline" });
                clear_own_host_thread(&app, &token);
                return;
            }
        };
        runtime.block_on(async {
            let mut next = first;
            loop {
                let params = host_session_params(&mut app.state::<AppState>().lock());
                let (ws, pairing) = next;
                if run_paired_host_session(&app, ws, pairing, params, &mut stop_rx).await {
                    break;
                }
                // Ended on its own (viewer left, network drop, rejection):
                // go straight back to waiting rather than going offline.
                emit(&app, "status_changed", StatusEvent { state: "connecting" });
                let still_hosting = || owns_host_thread(&app, &token);
                tokio::select! {
                    paired = register_host_until_paired(&app, &registration, still_hosting) => match paired {
                        Some(paired) => next = paired,
                        None => break,
                    },
                    _ = &mut stop_rx => break,
                }
            }
        });
        emit(&app, "status_changed", StatusEvent { state: "offline" });
        // Leave no stale handle preventing a later start_hosting call.
        clear_own_host_thread(&app, &token);
    })
}

#[tauri::command]
fn start_hosting(state: tauri::State<'_, AppState>, app: AppHandle) -> Result<(), String> {
    let reservation_token = Arc::new(());
    let registration = {
        let mut s = state.lock();
        // Check and reserve the hosting slot under this single lock so two
        // rapid calls (e.g. a StrictMode double mount) cannot both spawn.
        if !can_reserve_host_slot(s.host_thread.is_some(), s.host_starting.is_some()) {
            return Ok(());
        }
        s.host_starting = Some(HostReservation { token: reservation_token.clone(), task: None });
        HostRegistration { id: s.config.ensure_identity(), token: s.config.ensure_registration_token() }
    };
    emit(&app, "status_changed", StatusEvent { state: "connecting" });
    let task_token = reservation_token.clone();
    let task = tauri::async_runtime::spawn(async move {
        let still_reserved = || owns_current_host_reservation(&app, &task_token);
        let Some(first) = register_host_until_paired(&app, &registration, still_reserved).await else {
            return;
        };
        // Hold the state lock across spawning and installing the thread:
        // the thread's own cleanup needs this lock, so it cannot clear the
        // slot before its handle is installed.
        let app_state = app.state::<AppState>();
        let mut runtime_state = app_state.lock();
        if !owns_host_reservation(runtime_state.host_starting.as_ref(), &task_token) {
            // stop_hosting() released the reservation while we were
            // pairing; drop the socket instead of starting a session.
            return;
        }
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let join = spawn_host_thread(app.clone(), registration, first, stop_rx, task_token.clone());
        runtime_state.host_starting = None;
        runtime_state.host_thread = Some(HostThread { stop_tx, join, token: task_token.clone() });
    });
    // Keep the registration handle with the reservation so stop_hosting() can
    // abort a registration still retrying or waiting for a pairing. If the
    // reservation is already gone it was either handed off to the HostThread
    // (the task is finishing) or released by stop_hosting/disconnect before
    // the handle existed; abort so a retrying registration cannot outlive it.
    let mut runtime_state = state.lock();
    match runtime_state
        .host_starting
        .as_mut()
        .filter(|reservation| Arc::ptr_eq(&reservation.token, &reservation_token))
    {
        Some(reservation) => reservation.task = Some(task),
        None => task.abort(),
    }
    Ok(())
}

fn owns_current_host_reservation(app: &AppHandle, token: &Arc<()>) -> bool {
    let app_state = app.state::<AppState>();
    let runtime_state = app_state.lock();
    owns_host_reservation(runtime_state.host_starting.as_ref(), token)
}

fn owns_host_thread(app: &AppHandle, token: &Arc<()>) -> bool {
    let app_state = app.state::<AppState>();
    let runtime_state = app_state.lock();
    runtime_state
        .host_thread
        .as_ref()
        .is_some_and(|host_thread| Arc::ptr_eq(&host_thread.token, token))
}

/// Clear `host_thread` only if it still belongs to the thread identified by
/// `token`, so a finishing old session never clears a newer one.
fn clear_own_host_thread(app: &AppHandle, token: &Arc<()>) {
    let mut runtime_state = app.state::<AppState>().inner().lock();
    if runtime_state
        .host_thread
        .as_ref()
        .is_some_and(|host_thread| Arc::ptr_eq(&host_thread.token, token))
    {
        runtime_state.host_thread = None;
    }
}
#[tauri::command]
fn stop_hosting(state: tauri::State<'_, AppState>, app: AppHandle) {
    let (host_thread, pending_request, host_starting) = {
        let mut s = state.lock();
        (s.host_thread.take(), s.pending_request.take(), s.host_starting.take())
    };
    drop(pending_request);
    // Releasing the reservation also makes an in-flight registration task
    // bail out at hand-off if the abort lands after its last await point.
    if let Some(task) = host_starting.and_then(|reservation| reservation.task) {
        task.abort();
    }
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
    auth_code: Option<String>,
) -> Result<(), String> {
    if target_id.len() != 9 || !target_id.bytes().all(|b| b.is_ascii_digit()) {
        return Err("target ID must be 9 digits".into());
    }
    state
        .lock()
        .config
        .add_recent(&target_id)
        .map_err(|e| e.to_string())?;
    // A contact is only touched when it already exists; the regular recent-ID
    // history above continues to cover connections made without saving one.
    let touch_result = state.lock().contacts.touch(&target_id);
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
    let (relay_urls, viewer_id) = {
        let mut state = state.lock();
        (state.config.config.relay_urls.clone(), state.config.ensure_identity())
    };
    let target = target_id.clone();
    let task_app = app.clone();
    let viewer_stop = Arc::new(AtomicBool::new(false));
    {
        let mut runtime_state = state.lock();
        if !can_reserve_viewer_slot(
            runtime_state.viewer_task.is_some(),
            runtime_state.viewer_stop.is_some(),
        ) {
            return Err("viewer session is already active".into());
        }
        // Reserve the slot before spawning. `viewer_stop` is the reservation
        // marker while the JoinHandle is being created and installed.
        runtime_state.viewer_stop = Some(viewer_stop.clone());
    }
    let task_viewer_stop = viewer_stop.clone();
    let mut task = Some(tauri::async_runtime::spawn(async move {
        let result = async {
            // Retry state for the current outage: attempts so far and when the
            // outage began. Reset once an attempt actually streams media, so
            // every drop of a working session gets a full reconnect window.
            let mut reconnect_attempt = 0;
            let mut outage_started: Option<Instant> = None;
            let mut session_started = false;
            loop {
                if task_viewer_stop.load(Ordering::Relaxed) { break Ok(()); }
                emit(&task_app, "status_changed", StatusEvent { state: "connecting" });
                // Set once this attempt streams video; only used to restart the
                // reconnect window, never to infer an auth failure.
                let media_received = Arc::new(AtomicBool::new(false));
                // Clone per attempt: each iteration moves these into the session
                // task, so the originals must survive for the next reconnect.
                let password = password.clone();
                let auth_code = auth_code.clone();
                let attempt_result = async {
            let (_, mut ws) = host_core::relay_client::connect_best(&relay_urls).await?;
            host_core::relay_client::connect_to(&mut ws, &target, &viewer_id)
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
            task_app.state::<AppState>().lock().viewer_input = Some(input_tx);

            let frame_app = task_app.clone();
            let frame_media_received = media_received.clone();
            let _frame_task = AbortOnDrop::new(tauri::async_runtime::spawn(async move {
                while let Some(plaintext) = frame_rx.recv().await {
                    if plaintext.len() < 18 {
                        continue;
                    }
                    frame_media_received.store(true, Ordering::Relaxed);
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
                            data: b64(&plaintext[18..]),
                        },
                    );
                }
            }));

            let audio_app = task_app.clone();
            let audio_muted = task_app.state::<AppState>().lock().audio_muted.clone();
            let _audio_task = AbortOnDrop::new(tauri::async_runtime::spawn(async move {
                let mut decoder: Option<(u32, u8, opus::Decoder)> = None;
                while let Some(body) = audio_rx.recv().await {
                    if audio_muted.load(Ordering::Relaxed) { continue; }
                    if let Some(frame) = protocol::audio::AudioFrame::from_bytes(&body) {
                        let needs_new_decoder = decoder.as_ref().is_none_or(|(rate, channels, _)| *rate != frame.sample_rate || *channels != frame.channels);
                        if needs_new_decoder {
                            let channels = if frame.channels == 1 { opus::Channels::Mono } else { opus::Channels::Stereo };
                            decoder = opus::Decoder::new(frame.sample_rate, channels).ok().map(|decoder| (frame.sample_rate, frame.channels, decoder));
                        }
                        let Some((_, channels, decoder_ref)) = decoder.as_mut() else { continue };
                        let frame_capacity = usize::from(frame.frame_duration_ms) * frame.sample_rate as usize / 1_000 * usize::from(*channels);
                        let mut samples = vec![0i16; frame_capacity];
                        let Ok(sample_count) = decoder_ref.decode(&frame.payload, &mut samples, false) else { continue };
                        samples.truncate(sample_count * usize::from(*channels));
                        let mut pcm = Vec::with_capacity(samples.len() * 2);
                        for sample in &samples { pcm.extend_from_slice(&sample.to_le_bytes()); }
                        emit(&audio_app, "audio_frame", AudioFrameEvent { sample_rate: frame.sample_rate, channels: *channels, samples: b64(&pcm) });
                    }
                }
            }));

            let monitor_app = task_app.clone();
            let _monitor_task = AbortOnDrop::new(tauri::async_runtime::spawn(async move {
                while let Some(body) = monitor_rx.recv().await {
                    if let Some(Control::MonitorList { monitors }) = Control::from_bytes(&body) {
                        emit(&monitor_app, "monitor_list", MonitorListEvent { monitors });
                    }
                }
            }));

            let (data_ready_tx, data_ready_rx) = oneshot::channel();
            let mut session_task = AbortOnDrop::new(tauri::async_runtime::spawn(async move {
                host_core::roles::run_viewer_session_with_monitors_and_auth_code(
                    ws,
                    &password,
                    &session,
                    frame_tx,
                    input_rx,
                    Some(data_ready_tx),
                    Some(monitor_tx),
                    Some(audio_tx),
                    auth_code.clone(),
                )
                .await
            }));
            match data_ready_rx.await {
                Ok(handle) => {
                    activate_data_channel(&task_app, handle, target.clone());
                    emit(&task_app, "status_changed", StatusEvent { state: "in_session" });
                    if !session_started {
                        emit(
                            &task_app,
                            "session_started",
                            SessionStarted {
                                role: "viewer",
                                remote_id: target.clone(),
                            },
                        );
                        session_started = true;
                    }
                    match session_task.join().await {
                        Ok(result) => result.map_err(|error| error.to_string()),
                        Err(error) => Err(format!("viewer session task failed: {error}")),
                    }
                }
                Err(_) => match session_task.join().await {
                    Ok(result) => result.map_err(|error| error.to_string()),
                    Err(error) => Err(format!("viewer session task failed: {error}")),
                },
            }
                }
                .await;
                match attempt_result {
                    Ok(()) => break Ok(()),
                    Err(message) => {
                        clear_data_channel(&task_app);
                        task_app.state::<AppState>().lock().viewer_input = None;
                        let media_received = media_received.load(Ordering::Relaxed);
                        let reason = host_core::reconnect::classify_viewer_error(&message);
                        if reason == host_core::reconnect::DisconnectReason::AuthFailure {
                            // Only explicit signals land here (failed PAKE or the
                            // host's Control::Error rejection). Never retry: the
                            // password/2FA code will not get better (a TOTP code
                            // is single-use), and a host user already declined.
                            break Err(host_core::reconnect::auth_failure_message(&message, session_started));
                        }
                        if !host_core::reconnect::should_reconnect(reason) {
                            break Err(message);
                        }
                        if media_received {
                            reconnect_attempt = 0;
                            outage_started = None;
                        }
                        let elapsed = outage_started.get_or_insert_with(Instant::now).elapsed();
                        let Some(delay) = host_core::reconnect::viewer_retry_delay(reconnect_attempt, elapsed) else {
                            break Err(format!(
                                "{message}; gave up reconnecting after {}s",
                                host_core::reconnect::VIEWER_RECONNECT_WINDOW.as_secs()
                            ));
                        };
                        reconnect_attempt += 1;
                        emit(&task_app, "error", ErrorEvent { message: format!("viewer session dropped ({message}); reconnecting (attempt {reconnect_attempt})") });
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }.await;

        if let Err(message) = &result {
            let message = message.clone();
            emit(
                &task_app,
                "error",
                ErrorEvent {
                    message,
                },
            );
        }
        let app_state = task_app.state::<AppState>();
        let mut state = app_state.lock();
        state.viewer_input = None;
        state.viewer_task = None;
        state.viewer_stop = None;
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
    }));
    let installed = {
        let mut runtime_state = state.lock();
        if runtime_state
            .viewer_stop
            .as_ref()
            .is_some_and(|stop| Arc::ptr_eq(stop, &viewer_stop))
        {
            runtime_state.viewer_task = task.take();
            true
        } else {
            false
        }
    };
    if !installed {
        // disconnect() or an early task completion rolled back the
        // reservation before the handle could be installed.
        if let Some(task) = task {
            task.abort();
        }
        return Err("viewer session was stopped before it started".into());
    }
    Ok(())
}
#[tauri::command]
fn send_input(state: tauri::State<'_, AppState>, bytes: Vec<u8>) -> Result<(), String> {
    let sender = state.lock().viewer_input.clone();
    enqueue_input(sender, bytes)
}

#[tauri::command]
async fn select_monitor(state: tauri::State<'_, AppState>, id: u32) -> Result<(), String> {
    let (sender, sealer) = {
        let state = state.lock();
        (state.control_sender.clone(), state.control_sealer.clone())
    };
    let sender = sender.ok_or_else(|| "no active viewer session".to_owned())?;
    let sealer = sealer.ok_or_else(|| "control channel is not ready".to_owned())?;
    let body = Control::SelectMonitor { id }.to_bytes();
    let sealed = lock_unpoisoned(&sealer).seal(&[Tag::Control as u8], &body);
    sender.send_frame(Tag::Control, &sealed).await.map_err(|error| error.to_string())
}

#[tauri::command]
async fn send_auth_code(state: tauri::State<'_, AppState>, code: String) -> Result<(), String> {
    let (sender, sealer) = { let state = state.lock(); (state.control_sender.clone(), state.control_sealer.clone()) };
    let sender = sender.ok_or_else(|| "no active viewer session".to_owned())?;
    let sealer = sealer.ok_or_else(|| "control channel is not ready".to_owned())?;
    let sealed = lock_unpoisoned(&sealer).seal(&[Tag::Control as u8], &Control::AuthCode { code }.to_bytes());
    sender.send_frame(Tag::Control, &sealed).await.map_err(|error| error.to_string())
}

#[tauri::command]
fn set_clipboard_sync(state: tauri::State<'_, AppState>, enabled: bool) {
    state.lock().clipboard_enabled.store(enabled, Ordering::Relaxed);
}

#[tauri::command]
fn set_audio_muted(state: tauri::State<'_, AppState>, muted: bool) {
    state.lock().audio_muted.store(muted, Ordering::Relaxed);
}

#[tauri::command]
fn disconnect(state: tauri::State<'_, AppState>, app: AppHandle) {
    let (host_thread, viewer_task, host_starting) = {
        let mut s = state.lock();
        s.viewer_input = None;
        if let Some(stop) = &s.viewer_stop { stop.store(true, Ordering::Relaxed); }
        s.viewer_stop = None;
        (s.host_thread.take(), s.viewer_task.take(), s.host_starting.take())
    };
    if let Some(viewer_task) = viewer_task {
        // Best-effort Control::Bye so the host ends its session cleanly, then
        // abort. Done off the command thread so disconnect never stalls the UI;
        // bounded to ~200 ms so a wedged writer can't keep the task alive.
        let (sender, sealer) = { let s = state.lock(); (s.control_sender.clone(), s.control_sealer.clone()) };
        tauri::async_runtime::spawn(async move {
            if let (Some(sender), Some(sealer)) = (sender, sealer) {
                let bye = Control::Bye { session: String::new() }.to_bytes();
                let sealed = sealer.lock().unwrap_or_else(|error| error.into_inner()).seal(&[Tag::Control as u8], &bye);
                let sent = tokio::time::timeout(Duration::from_millis(150), sender.send_frame(Tag::Control, &sealed)).await;
                if matches!(sent, Ok(Ok(_))) {
                    // Let the WebSocket writer flush before the abort tears it down.
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
            viewer_task.abort();
        });
    }
    // Cancel an in-flight registration too, or the reservation would block
    // every later start_hosting call while the task waits on the relay.
    if let Some(task) = host_starting.and_then(|reservation| reservation.task) {
        task.abort();
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
                host_starting: None,
                viewer_task: None,
                viewer_stop: None,
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
            list_allowed,
            add_allowed,
            remove_allowed,
            add_contact,
            remove_contact,
            touch_contact,
            set_relay,
            get_unattended_status,
            set_unattended_access,
            set_unattended_2fa,
            permission_status,
            open_permission_settings,
            respond_to_request,
            start_hosting,
            stop_hosting,
            connect,
            disconnect,
            send_input,
            select_monitor,
            send_auth_code,
            set_clipboard_sync,
            set_audio_muted,
            send_file,
            respond_file,
            save_recording,
            set_relays,
            list_relays
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

    #[test]
    fn recording_filename_avoids_existing_file() {
        let directory = std::env::temp_dir().join(format!(
            "opendesk-viewer-recording-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("capture.webm"), b"existing").unwrap();

        let path = next_recording_path(&directory, "capture").unwrap();

        assert_eq!(path.file_name().and_then(|name| name.to_str()), Some("capture (2).webm"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recording_size_cap_rejects_oversized_input() {
        assert!(validate_recording_size_len(MAX_RECORDING_SIZE).is_ok());
        let error = validate_recording_size_len(MAX_RECORDING_SIZE + 1).unwrap_err();
        assert!(error.contains("maximum size"));
    }

    #[test]
    fn viewer_slot_reservation_fails_when_already_reserved() {
        assert!(can_reserve_viewer_slot(false, false));
        assert!(!can_reserve_viewer_slot(true, false));
        assert!(!can_reserve_viewer_slot(false, true));
    }

    #[test]
    fn host_slot_reservation_fails_while_starting_or_running() {
        assert!(can_reserve_host_slot(false, false));
        assert!(!can_reserve_host_slot(true, false));
        assert!(!can_reserve_host_slot(false, true));
        assert!(!can_reserve_host_slot(true, true));
    }

    #[test]
    fn host_reservation_is_owned_only_by_its_token() {
        let token = Arc::new(());
        let other = Arc::new(());
        let reservation = HostReservation { token: token.clone(), task: None };
        assert!(owns_host_reservation(Some(&reservation), &token));
        assert!(!owns_host_reservation(Some(&reservation), &other));
        assert!(!owns_host_reservation(None, &token));
    }

    #[test]
    fn second_pending_request_is_rejected_and_first_kept() {
        let mut slot = None;
        let (first_tx, first_rx) = oneshot::channel();
        assert!(admit_pending_request(&mut slot, first_tx).is_ok());

        let (second_tx, mut second_rx) = oneshot::channel();
        let rejected = admit_pending_request(&mut slot, second_tx).unwrap_err();
        rejected.send(false).unwrap();
        assert_eq!(second_rx.try_recv(), Ok(false));

        // The first requester still receives the user's decision.
        slot.take().unwrap().send(true).unwrap();
        assert_eq!(first_rx.blocking_recv(), Ok(true));
    }

    #[test]
    fn stale_pending_request_does_not_block_new_one() {
        let mut slot = None;
        let (stale_tx, stale_rx) = oneshot::channel::<bool>();
        assert!(admit_pending_request(&mut slot, stale_tx).is_ok());
        drop(stale_rx);

        let (fresh_tx, fresh_rx) = oneshot::channel();
        assert!(admit_pending_request(&mut slot, fresh_tx).is_ok());
        slot.take().unwrap().send(true).unwrap();
        assert_eq!(fresh_rx.blocking_recv(), Ok(true));
    }

    #[test]
    fn poisoned_inner_mutex_remains_usable() {
        let mutex = Mutex::new(Some(7_u64));
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = mutex.lock().unwrap();
            panic!("poison the test mutex");
        }));
        assert!(mutex.is_poisoned());
        assert_eq!(*lock_unpoisoned(&mutex), Some(7));
    }

    #[test]
    fn poisoned_app_state_lock_remains_usable() {
        let directory = std::env::temp_dir().join(format!(
            "opendesk-viewer-lock-test-{}",
            std::process::id()
        ));
        let app_state = AppState(Mutex::new(RuntimeState {
            config: ConfigStore::load(&directory).unwrap(),
            contacts: ContactStore::load(&directory).unwrap(),
            password: None,
            host_thread: None,
            host_starting: None,
            viewer_task: None,
            viewer_stop: None,
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
        }));

        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = app_state.lock();
            panic!("poison the test mutex");
        }));

        let guard = app_state.lock();
        assert!(guard.viewer_task.is_none());
        drop(guard);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn abort_on_drop_aborts_child_task() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (started_tx, started_rx) = oneshot::channel();
            let ticks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let task_ticks = ticks.clone();
            let task = tauri::async_runtime::spawn(async move {
                started_tx.send(()).unwrap();
                loop {
                    task_ticks.fetch_add(1, Ordering::Relaxed);
                    tokio::task::yield_now().await;
                }
            });
            let guard = AbortOnDrop::new(task);
            started_rx.await.unwrap();
            tokio::task::yield_now().await;
            let ticks_at_drop = ticks.load(Ordering::Relaxed);
            drop(guard);
            for _ in 0..3 {
                tokio::task::yield_now().await;
            }
            assert_eq!(ticks.load(Ordering::Relaxed), ticks_at_drop);
        });
    }
}
