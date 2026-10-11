//! Hosting: relay registration, pairing requests, and the host session loop.

use super::*;

/// The hosting slot is free only when no host thread is running and no
/// `start_hosting` registration is in flight.
pub(crate) fn can_reserve_host_slot(host_thread_running: bool, host_starting: bool) -> bool {
    !host_thread_running && !host_starting
}

/// Whether `token` still owns the in-flight hosting reservation.
pub(crate) fn owns_host_reservation(reservation: Option<&HostReservation>, token: &Arc<()>) -> bool {
    reservation.is_some_and(|reservation| Arc::ptr_eq(&reservation.token, token))
}

/// Admit `decision_tx` as the pending pairing request. Only one request may
/// wait for a user decision: if a live one is already pending, the newcomer is
/// handed back so the caller can reject it (the first keeps its UI prompt). A
/// stale sender whose requester already went away does not block new requests.
pub(crate) fn admit_pending_request(
    slot: &mut Option<oneshot::Sender<bool>>,
    decision_tx: oneshot::Sender<bool>,
) -> Result<(), oneshot::Sender<bool>> {
    if slot.as_ref().is_some_and(|existing| !existing.is_closed()) {
        return Err(decision_tx);
    }
    *slot = Some(decision_tx);
    Ok(())
}

#[tauri::command]
pub(crate) fn respond_to_request(
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

/// Per-session host credentials and policy. Read fresh for every pairing so a
/// rotated password or a changed unattended/2FA setting applies to the next
/// session of a long-lived hosting loop instead of the value from when
/// hosting started.
pub(crate) struct HostSessionParams {
    pub(crate) password: String,
    pub(crate) unattended_enabled: bool,
    pub(crate) twofa_enabled: bool,
    pub(crate) totp_secret: Option<String>,
}

pub(crate) fn host_session_params(s: &mut RuntimeState) -> HostSessionParams {
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

/// Wakes hosting loops when the saved relay list changes, so a parked
/// registration moves to the new list right away instead of at its next
/// renewal.
fn relays_changed() -> &'static tokio::sync::Notify {
    static RELAYS_CHANGED: std::sync::OnceLock<tokio::sync::Notify> = std::sync::OnceLock::new();
    RELAYS_CHANGED.get_or_init(tokio::sync::Notify::new)
}

/// Call after saving a new relay list: every hosting loop re-registers.
pub(crate) fn notify_relays_changed() {
    relays_changed().notify_waiters();
}

/// The relay identity a hosting loop registers under. It is stable for the
/// loop's lifetime: re-registering with the same token is accepted by the
/// relay's ownership lease, so a host can come back after every session.
#[derive(Clone)]
pub(crate) struct HostRegistration {
    pub(crate) id: String,
    pub(crate) token: String,
}

/// Connect to the relay, register, and wait for a viewer to pair, retrying
/// with capped exponential backoff until a pairing arrives. Returns `None`
/// once `still_hosting` reports that this loop no longer owns the hosting
/// slot. Callers may also cancel by dropping the future (task abort or a
/// `select!` on the host thread's stop signal).
///
/// Registration uses the saved relay list in order (first reachable wins),
/// and starts over at once when [`notify_relays_changed`] reports a new list.
///
/// Status: "online" when a registration is parked on the relay; "connecting"
/// (plus one error event per failure streak) while registration is failing.
/// A healthy registration that the relay expires (its pairing timeout) is
/// renewed silently so the UI stays "online".
pub(crate) async fn register_host_until_paired(
    app: &AppHandle,
    registration: &HostRegistration,
    still_hosting: impl Fn() -> bool,
) -> Option<(host_core::relay_client::RelayWebSocket, host_core::relay_client::Pairing)> {
    let mut backoff = host_core::reconnect::HostRegistrationBackoff::new();
    let mut online = false;
    loop {
        // Created before the attempt so a change saved while connecting is
        // not missed (a Notified future counts from creation).
        let relay_change = relays_changed().notified();
        tokio::pin!(relay_change);
        let delay = backoff.delay();
        if !delay.is_zero() {
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = &mut relay_change => {
                    backoff = host_core::reconnect::HostRegistrationBackoff::new();
                    continue;
                }
            }
        }
        if !still_hosting() {
            return None;
        }
        // Re-read per attempt so a relay change applies to the next registration.
        let relay_urls = app.state::<AppState>().lock().config.config.relay_urls.clone();
        let failure = match host_core::relay_client::register_host(&relay_urls, &registration.id, &registration.token).await {
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
                let paired = tokio::select! {
                    paired = host_core::relay_client::await_pairing(&mut ws) => paired,
                    _ = &mut relay_change => {
                        log::info!("relay list changed; re-registering");
                        backoff = host_core::reconnect::HostRegistrationBackoff::new();
                        continue;
                    }
                };
                match paired {
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
pub(crate) async fn run_paired_host_session(
    app: &AppHandle,
    ws: host_core::relay_client::RelayWebSocket,
    pairing: host_core::relay_client::Pairing,
    params: HostSessionParams,
    stop_rx: &mut oneshot::Receiver<()>,
) -> HostSessionEnd {
    let host_core::relay_client::Pairing { session: ice_session, peer_id } = pairing;
    // Set once the viewer has passed the password (and 2FA) checks, which is
    // when host-core asks for approval.
    let authenticated = Arc::new(AtomicBool::new(false));
    let authenticated_flag = authenticated.clone();
    let HostSessionParams { password, unattended_enabled, twofa_enabled, totp_secret } = params;
    // The viewer's peer ID comes from the relay's pairing message and is not
    // authenticated, so an allow-listed ID never skips approval; the prompt
    // only marks it as a known device. Unattended access (its own password,
    // plus optional TOTP) is the only path without a prompt.
    let known = app.state::<AppState>().lock().config.is_allowed(&peer_id);
    let remote_id = peer_id.clone();
    let approve_app = app.clone();
    let (data_ready_tx, data_ready_rx) = oneshot::channel();
    let approve = move || async move {
        authenticated_flag.store(true, Ordering::Relaxed);
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
            peer_id: peer_id.clone(), fingerprint: "pending".into(), known,
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
    if result.is_none() {
        // The host user stopped: tell the viewer explicitly so it ends
        // cleanly instead of treating the close as a drop and reconnecting.
        // The session (and its writer) is still alive here.
        send_host_bye(app).await;
    }
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
    HostSessionEnd { stopped: result.is_none(), authenticated: authenticated.load(Ordering::Relaxed) }
}

/// Best-effort sealed `Control::Bye` on the active session's control channel.
async fn send_host_bye(app: &AppHandle) {
    let (sender, sealer) = {
        let state = app.state::<AppState>();
        let state = state.lock();
        (state.control_sender.clone(), state.control_sealer.clone())
    };
    if let (Some(sender), Some(sealer)) = (sender, sealer) {
        host_core::roles::send_bye(&sender, &sealer).await;
    }
}

/// Cleans up after a host thread that panicked (e.g. inside encoder
/// initialization): without it `host_thread` stays `Some` and every later
/// `start_hosting` silently does nothing. Normal exits clean up themselves.
/// Only effective where panics unwind (release builds abort on panic).
struct HostThreadPanicGuard {
    app: AppHandle,
    token: Arc<()>,
}

impl Drop for HostThreadPanicGuard {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            return;
        }
        clear_data_channel(&self.app);
        clear_own_host_thread(&self.app, &self.token);
        emit(&self.app, "error", ErrorEvent { message: "hosting stopped unexpectedly; start hosting again".into() });
        emit(&self.app, "session_ended", SessionEnded { reason: "host session failed".into() });
        emit(&self.app, "status_changed", StatusEvent { state: "offline" });
    }
}

pub(crate) struct HostSessionEnd {
    /// The user stopped hosting; leave the hosting loop.
    pub(crate) stopped: bool,
    /// The viewer got past the password/2FA checks.
    pub(crate) authenticated: bool,
}

/// Limits password guessing against a host. The main defence is rotation:
/// the one-time session password changes after every authenticated session
/// and after every third consecutive failed one, so a guesser gets at most
/// three tries per password (3 in 900,000). On top of that a short delay
/// (2 s, 4 s, then 5 s) slows repeated failures. The delay is capped low
/// because the host is unregistered while it waits: a long delay would let
/// anyone who knows the host ID keep it offline with a few bad logins.
#[derive(Default)]
pub(crate) struct AuthBackoff {
    consecutive_failures: u32,
}

/// What the hosting loop does after a session ends.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct AfterSession {
    /// Wait this long before registering again.
    pub(crate) delay: Duration,
    /// Replace the one-time session password.
    pub(crate) rotate_password: bool,
}

impl AuthBackoff {
    const FREE_ATTEMPTS: u32 = 3;
    const ROTATE_EVERY_FAILURES: u32 = 3;
    const MAX_DELAY: Duration = Duration::from_secs(5);

    pub(crate) fn after_session(&mut self, authenticated: bool) -> AfterSession {
        if authenticated {
            self.consecutive_failures = 0;
            return AfterSession { delay: Duration::ZERO, rotate_password: true };
        }
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        let rotate_password = self.consecutive_failures.is_multiple_of(Self::ROTATE_EVERY_FAILURES);
        let over = self.consecutive_failures.saturating_sub(Self::FREE_ATTEMPTS);
        let delay = if over == 0 {
            Duration::ZERO
        } else {
            Duration::from_secs(1u64 << over.min(6)).min(Self::MAX_DELAY)
        };
        AfterSession { delay, rotate_password }
    }
}

/// Each host session runs on this dedicated thread's current-thread runtime.
/// The thread is the hosting loop:
/// after a session ends on its own it re-registers with the relay under the
/// same ID/token and waits for the next pairing, so a dropped viewer can
/// reconnect. Only the stop signal (stop_hosting/disconnect) or losing
/// ownership of `host_thread` ends the loop.
pub(crate) fn spawn_host_thread(
    app: AppHandle,
    registration: HostRegistration,
    first: (host_core::relay_client::RelayWebSocket, host_core::relay_client::Pairing),
    mut stop_rx: oneshot::Receiver<()>,
    token: Arc<()>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let _panic_guard = HostThreadPanicGuard { app: app.clone(), token: token.clone() };
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
            let mut backoff = AuthBackoff::default();
            loop {
                let params = host_session_params(&mut app.state::<AppState>().lock());
                let (ws, pairing) = next;
                let ended = run_paired_host_session(&app, ws, pairing, params, &mut stop_rx).await;
                let AfterSession { delay, rotate_password } = backoff.after_session(ended.authenticated);
                if rotate_password {
                    // A finished session's password must not let anyone back
                    // in, and a guesser gets a fresh target every 3 misses.
                    rotate_session_password(&app);
                }
                if ended.stopped {
                    break;
                }
                if !delay.is_zero() {
                    log::warn!("repeated failed authentication; waiting {}s before accepting the next request", delay.as_secs());
                    tokio::select! {
                        _ = tokio::time::sleep(delay) => {}
                        _ = &mut stop_rx => break,
                    }
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
pub(crate) fn start_hosting(state: tauri::State<'_, AppState>, app: AppHandle) -> Result<(), String> {
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

pub(crate) fn owns_current_host_reservation(app: &AppHandle, token: &Arc<()>) -> bool {
    let app_state = app.state::<AppState>();
    let runtime_state = app_state.lock();
    owns_host_reservation(runtime_state.host_starting.as_ref(), token)
}

pub(crate) fn owns_host_thread(app: &AppHandle, token: &Arc<()>) -> bool {
    let app_state = app.state::<AppState>();
    let runtime_state = app_state.lock();
    runtime_state
        .host_thread
        .as_ref()
        .is_some_and(|host_thread| Arc::ptr_eq(&host_thread.token, token))
}

/// Clear `host_thread` only if it still belongs to the thread identified by
/// `token`, so a finishing old session never clears a newer one.
pub(crate) fn clear_own_host_thread(app: &AppHandle, token: &Arc<()>) {
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
pub(crate) fn stop_hosting(state: tauri::State<'_, AppState>, app: AppHandle) {
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
