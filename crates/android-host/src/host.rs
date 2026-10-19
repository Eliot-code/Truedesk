//! Shares the phone: registers with a relay and runs a host session for
//! each viewer that pairs, with the phone's screen as the video and the
//! viewer's input mapped to touch gestures.
//!
//! The app drives this through a [`HostController`]: it starts and stops
//! hosting, answers the allow/deny prompt, and hears status and prompts
//! through a [`HostEvents`] sink it provides.

use crate::capture::FramePusher;
use crate::input::{ActionQueue, GestureInjector};
use host_core::reconnect::HostRegistrationBackoff;
use host_core::relay_client::{
    await_pairing, is_id_in_use, is_id_taken, refusal_message, register_host, settle_registration,
    Pairing, REGISTRATION_SETTLE,
};
use host_core::roles::{run_platform_host_session, DataHandle, SessionGates};
use std::sync::{Arc, Mutex};
use tokio::sync::{oneshot, Notify};

#[cfg(test)]
mod tests;

/// What the app needs to host: where to reach relays, who this phone is, and
/// the one-time or fixed password a viewer must enter.
#[derive(Clone)]
pub struct HostConfig {
    pub relay_urls: Vec<String>,
    pub id: String,
    pub token: String,
    pub password: String,
    /// Accept viewers without asking. Off by default: someone holding the
    /// phone taps Allow for each one.
    pub unattended: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostStatus {
    /// Trying to reach a relay.
    Connecting,
    /// Registered and waiting for a viewer.
    Online,
    /// A viewer is connected.
    InSession,
}

impl HostStatus {
    pub fn name(self) -> &'static str {
        match self {
            Self::Connecting => "connecting",
            Self::Online => "online",
            Self::InSession => "in_session",
        }
    }
}

/// Something the app should hear about while hosting.
#[derive(Clone, Debug, PartialEq)]
pub enum HostEvent {
    Status(HostStatus),
    /// A viewer wants in; the app shows a prompt and calls
    /// [`HostController::answer`].
    Request { peer_id: String },
    SessionStarted { peer_id: String },
    SessionEnded,
    /// The relay gave this phone's ID to someone else; the app should reset
    /// the identity and try again.
    IdentityInUse,
    Error(String),
}

impl HostEvent {
    /// The event as one line of JSON for the app's UI, e.g.
    /// `{"t":"status","s":"online"}` or `{"t":"request","peer":"123456789"}`.
    pub fn to_json(&self) -> String {
        match self {
            Self::Status(status) => format!(r#"{{"t":"status","s":"{}"}}"#, status.name()),
            Self::Request { peer_id } => format!(r#"{{"t":"request","peer":{}}}"#, json_string(peer_id)),
            Self::SessionStarted { peer_id } => {
                format!(r#"{{"t":"session_started","peer":{}}}"#, json_string(peer_id))
            }
            Self::SessionEnded => r#"{"t":"session_ended"}"#.to_owned(),
            Self::IdentityInUse => r#"{"t":"identity_in_use"}"#.to_owned(),
            Self::Error(message) => format!(r#"{{"t":"error","message":{}}}"#, json_string(message)),
        }
    }
}

/// Minimal JSON string escaping (the peer id and error messages).
fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if (ch as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// Where host events go. The app implements this (it forwards to the UI).
pub trait HostEvents: Send + Sync {
    fn on(&self, event: HostEvent);
}

impl<F: Fn(HostEvent) + Send + Sync> HostEvents for F {
    fn on(&self, event: HostEvent) {
        self(event)
    }
}

/// Handle the app keeps while hosting: pushes screen frames in, takes touch
/// actions out, answers prompts, and stops.
#[derive(Clone)]
pub struct HostController {
    frames: FramePusher,
    actions: ActionQueue,
    stop: Arc<Notify>,
    stopped: Arc<Mutex<bool>>,
    pending: Arc<Mutex<Option<oneshot::Sender<bool>>>>,
}

impl HostController {
    /// The screen frames sink. The app's capture callback pushes to it.
    pub fn frames(&self) -> FramePusher {
        self.frames.clone()
    }

    /// The touch actions to carry out. The app's accessibility service takes
    /// from it.
    pub fn actions(&self) -> ActionQueue {
        self.actions.clone()
    }

    /// Answers the current allow/deny prompt. A second call, or one with no
    /// prompt waiting, is ignored.
    pub fn answer(&self, allow: bool) {
        if let Some(tx) = self.pending.lock().unwrap_or_else(|p| p.into_inner()).take() {
            let _ = tx.send(allow);
        }
    }

    /// Stops hosting: ends any session and the registration loop. The
    /// [`run`] future returns soon after.
    pub fn stop(&self) {
        *self.stopped.lock().unwrap_or_else(|p| p.into_inner()) = true;
        self.answer(false);
        self.stop.notify_waiters();
    }

    fn is_stopped(&self) -> bool {
        *self.stopped.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// Creates a controller and the loop that drives it. Spawn the returned
/// future; it ends when [`HostController::stop`] is called.
pub fn host(config: HostConfig, events: Arc<dyn HostEvents>) -> (HostController, impl std::future::Future<Output = ()>) {
    let controller = HostController {
        frames: FramePusher::new(),
        actions: ActionQueue::new(),
        stop: Arc::new(Notify::new()),
        stopped: Arc::new(Mutex::new(false)),
        pending: Arc::new(Mutex::new(None)),
    };
    let run = run(config, controller.clone(), events);
    (controller, run)
}

async fn run(config: HostConfig, controller: HostController, events: Arc<dyn HostEvents>) {
    let mut backoff = HostRegistrationBackoff::new();
    let mut online = false;
    while !controller.is_stopped() {
        let delay = backoff.delay();
        if !delay.is_zero() {
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = controller.stop.notified() => {}
            }
            if controller.is_stopped() {
                break;
            }
        }
        if !online {
            events.on(HostEvent::Status(HostStatus::Connecting));
        }
        let failure = match register_host(&config.relay_urls, &config.id, &config.token).await {
            Err(error) => error,
            Ok((_relay, mut ws)) => {
                let settled = tokio::select! {
                    settled = settle_registration(&mut ws, REGISTRATION_SETTLE) => settled,
                    _ = controller.stop.notified() => break,
                };
                match settled {
                    Err(error) if is_id_taken(&error) || is_id_in_use(&error) => {
                        events.on(HostEvent::IdentityInUse);
                        events.on(HostEvent::Error("this device ID is in use on the relay".into()));
                        backoff.record_failure();
                        online = false;
                        continue;
                    }
                    Err(error) => error,
                    Ok(early) => {
                        backoff = HostRegistrationBackoff::new();
                        if !online {
                            online = true;
                            events.on(HostEvent::Status(HostStatus::Online));
                        }
                        let paired = match early {
                            Some(pairing) => Ok(pairing),
                            None => tokio::select! {
                                paired = await_pairing(&mut ws) => paired,
                                _ = controller.stop.notified() => break,
                            },
                        };
                        match paired {
                            Ok(pairing) => {
                                serve(&config, &controller, events.as_ref(), ws, pairing).await;
                                online = false;
                                continue;
                            }
                            // A parked registration that ended normally (the
                            // relay's pairing timeout) just re-registers.
                            Err(_) => continue,
                        }
                    }
                }
            }
        };
        online = false;
        backoff.record_failure();
        if backoff.consecutive_failures() == 1 {
            let message = refusal_message(&failure).map(str::to_owned).unwrap_or(failure);
            events.on(HostEvent::Error(message));
        }
    }
    controller.frames.clear();
    controller.actions.clear();
}

/// Runs one paired session end to end.
async fn serve(
    config: &HostConfig,
    controller: &HostController,
    events: &dyn HostEvents,
    ws: host_core::relay_client::RelayWebSocket,
    pairing: Pairing,
) {
    let peer_id = pairing.peer_id.clone();
    let actions = controller.actions.clone();
    actions.clear();
    let injector_actions = actions.clone();
    let injector_factory = move || Box::new(GestureInjector::new(injector_actions)) as Box<dyn host_core::inject::InputInjector>;

    let frames = controller.frames.clone();
    let capture_cell = Arc::new(Mutex::new(Some(frames.capturer())));
    let capture_factory = move || capture_cell.lock().unwrap_or_else(|p| p.into_inner()).take();

    let pending = Arc::clone(&controller.pending);
    let request_peer = peer_id.clone();
    let unattended = config.unattended;
    let approve = || async move {
        let allowed = if unattended {
            true
        } else {
            let (tx, rx) = oneshot::channel();
            *pending.lock().unwrap_or_else(|p| p.into_inner()) = Some(tx);
            events.on(HostEvent::Request { peer_id: request_peer.clone() });
            rx.await.unwrap_or(false)
        };
        if allowed {
            events.on(HostEvent::SessionStarted { peer_id: request_peer });
        }
        allowed
    };

    // No data channel on the phone host yet (clipboard, files, tunnels are
    // the desktop host's); the session runs without one.
    let data_ready: Option<oneshot::Sender<DataHandle>> = None;
    let session = run_platform_host_session(
        ws, &config.password, &pairing.session, capture_factory, injector_factory,
        unattended, approve, data_ready, SessionGates::default(),
    );
    tokio::pin!(session);

    let result = tokio::select! {
        result = &mut session => Some(result),
        _ = controller.stop.notified() => None,
    };

    // Whatever happened, drop any unanswered prompt and clear pending touches.
    controller.answer(false);
    actions.clear();
    if let Some(Err(error)) = &result {
        // A viewer that failed the password or left is routine; log it and
        // carry on waiting for the next one.
        log::info!("phone host session with {peer_id} ended: {error}");
    }
    events.on(HostEvent::SessionEnded);
}
