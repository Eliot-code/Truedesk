mod config;
mod contacts;
mod permissions;
mod unattended;
mod events;
mod recording;
mod files;
mod host;
mod viewer;

use config::ConfigStore;
use contacts::{Contact, ContactStore};
use futures_util::StreamExt;
use host_core::{clipboard::{ArboardBackend, ClipboardSync}, filexfer::{ConsentDecision, FileReceiver, FileSink, FileSource, FileSender}, roles::DataHandle, transport::reliable::ReliableSender};
use protocol::{control::{Control, MonitorInfo}, crypto::{Opener, Sealer}, datamsg::DataMsg, filexfer::FileMsg, frame, Tag};
use serde::Serialize;
use std::{collections::HashMap, fs, fs::OpenOptions, io, path::{Path, PathBuf}, sync::{atomic::{AtomicBool, Ordering}, mpsc as std_mpsc, Arc, Mutex}, time::{Duration, Instant}};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{mpsc, oneshot};
pub(crate) use events::*;
pub(crate) use recording::*;
pub(crate) use files::*;
pub(crate) use host::*;
pub(crate) use viewer::*;

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

/// Each host session runs with its own current-thread Tokio runtime on a
/// dedicated OS thread (input injection has its own thread inside host-core;
/// see `roles.rs`). The stop
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
        // Stdout plus a rotating file in the OS log dir (macOS:
        // ~/Library/Logs/com.opendesk.app/) for bug reports.
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                // ICE logs every candidate check at info; keep only problems.
                .level_for("webrtc_ice", log::LevelFilter::Warn)
                .level_for("webrtc_util", log::LevelFilter::Warn)
                .max_file_size(5 * 1024 * 1024)
                .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepOne)
                .build(),
        )
        .manage(Recordings::default())
        .setup(|app| {
            let app_data_dir = app
                .path()
                .app_data_dir()
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            // Debug builds only: a second instance with its own identity, for
            // host+viewer testing on one machine (see docs/TESTING.md).
            #[cfg(debug_assertions)]
            let app_data_dir = std::env::var_os("OPENDESK_DATA_DIR").map(PathBuf::from).unwrap_or(app_data_dir);
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
            start_recording,
            append_recording,
            finish_recording,
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
    fn recording_sink_streams_chunks_to_disk() {
        let directory = std::env::temp_dir().join(format!("opendesk-recording-sink-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();

        let mut sink = RecordingSink::create(&directory, "session").unwrap();
        sink.append(b"abc").unwrap();
        sink.append(b"def").unwrap();
        let path = sink.finish().unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"abcdef");
        assert_eq!(path.file_name().and_then(|name| name.to_str()), Some("session.webm"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recording_sink_enforces_size_cap_across_chunks() {
        let directory = std::env::temp_dir().join(format!("opendesk-recording-cap-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();

        let mut sink = RecordingSink::create(&directory, "capped").unwrap();
        sink.written = MAX_RECORDING_SIZE - 2;
        assert!(sink.append(b"ab").is_ok());
        assert!(sink.append(b"c").unwrap_err().contains("maximum size"));
        fs::remove_dir_all(directory).unwrap();
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
            drop(guard);
            // The task runs on Tauri's multi-threaded runtime, so it may tick
            // once more while the abort lands. Let it settle, then it must
            // stay stopped.
            tokio::time::sleep(Duration::from_millis(50)).await;
            let ticks_after_abort = ticks.load(Ordering::Relaxed);
            tokio::time::sleep(Duration::from_millis(50)).await;
            assert_eq!(ticks.load(Ordering::Relaxed), ticks_after_abort);
        });
    }
}
