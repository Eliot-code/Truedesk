//! Host-role session orchestration.

use crate::{
    audio,
    capture::{ScapCapturer, ScreenCapturer},
    encode::{self, stream::{self, KeyframePolicy, STREAM_FPS}, Encoder, VideoEncoder},
    inject::{EnigoInjector, InputInjector, NoopInjector},
    transport::{self, RelayIceSignaling, RelayTransport, ReliableChannel, Transport, TransportReceiver, TransportSender},
};
use crate::reconnect::{classify_viewer_error, reject_code, rejection_error, DisconnectReason};
use protocol::{
    control::Control,
    crypto::{Opener, Sealer},
    input::InputEvent,
    kex::derive_keys,
    Tag,
};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, error::Error, fmt::Display, future::Future, sync::{atomic::{AtomicU32, AtomicU64, Ordering}, Arc, Mutex, OnceLock}, time::{Duration, Instant}};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::WebSocketStream;

const KEX_MSG: u8 = 0x01;
const KEX_CONFIRM: u8 = 0x02;
/// A paired peer must complete both SPAKE2 exchanges promptly; this bounds
/// resources held by connections that never authenticate.
const KEX_DEADLINE: Duration = Duration::from_secs(10);

/// ICE role invariant: the host controls nomination and the viewer accepts it.
/// These values MUST differ for ICE to nominate a connection.
const HOST_ICE_CONTROLLING: bool = true;
const VIEWER_ICE_CONTROLLING: bool = false;

/// Whether a successfully authenticated session may bypass manual approval.
pub fn should_auto_accept(unattended_enabled: bool, pake_succeeded: bool) -> bool {
    unattended_enabled && pake_succeeded
}

/// Allow-list approval only skips the UI dialog; PAKE authentication still runs first.
pub fn should_auto_accept_allowlisted(is_allowed: bool) -> bool { is_allowed }

pub fn should_auto_accept_with_2fa(unattended_enabled: bool, pake_succeeded: bool, twofa_enabled: bool, code_valid: bool) -> bool {
    unattended_enabled && pake_succeeded && (!twofa_enabled || code_valid)
}

pub fn totp_is_valid(secret: &str, code: &str, unix_time: u64) -> bool {
    if code.len() != 6 || !code.bytes().all(|byte| byte.is_ascii_digit()) { return false; }
    // Fail closed on a malformed secret. Authenticator apps base32-decode the
    // enrolled secret to get the HMAC key; falling back to the raw ASCII bytes
    // would compute TOTP over the wrong key and could never match, so a decode
    // failure must reject rather than silently use a different key.
    let Some(decoded) = decode_base32(secret) else { return false; };
    [unix_time.saturating_sub(30), unix_time, unix_time.saturating_add(30)]
        .into_iter()
        .fold(false, |matched, time| {
            matched | constant_time_code_eq(
                &totp_lite::totp_custom::<totp_lite::Sha1>(30, 6, &decoded, time),
                code,
            )
        })
}

fn constant_time_code_eq(candidate: &str, supplied: &str) -> bool {
    if candidate.len() != 6 || supplied.len() != 6 { return false; }
    let mut difference = 0u8;
    for (left, right) in candidate.as_bytes().iter().zip(supplied.as_bytes()) {
        difference |= left ^ right;
    }
    difference == 0
}

static ACCEPTED_TOTP_STEPS: OnceLock<Mutex<HashMap<[u8; 32], u64>>> = OnceLock::new();

/// Validate a TOTP and atomically consume its matching step for this host's
/// secret. The process-wide map is keyed by a SHA-256 digest of the secret,
/// so reconnects to the same host share replay state; it is not durable across
/// process restarts and does not coordinate separate host processes.
fn totp_is_valid_once(secret: &str, code: &str, unix_time: u64) -> bool {
    if code.len() != 6 || !code.bytes().all(|byte| byte.is_ascii_digit()) { return false; }
    let Some(decoded) = decode_base32(secret) else { return false; };
    let current_step = unix_time / 30;
    let candidates = [
        current_step.saturating_sub(1),
        current_step,
        current_step.saturating_add(1),
    ];
    let mut matched_step = None;
    for step in candidates {
        let candidate = totp_lite::totp_custom::<totp_lite::Sha1>(30, 6, &decoded, step * 30);
        if constant_time_code_eq(&candidate, code) && matched_step.is_none() {
            matched_step = Some(step);
        }
    }
    let Some(step) = matched_step else { return false; };
    let key: [u8; 32] = Sha256::digest(secret.as_bytes()).into();
    let cache = ACCEPTED_TOTP_STEPS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut accepted = cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if accepted.get(&key).is_some_and(|last| step <= *last) { return false; }
    accepted.insert(key, step);
    true
}

fn decode_base32(value: &str) -> Option<Vec<u8>> {
    let mut output = Vec::new();
    let mut buffer = 0u32;
    let mut bits = 0u8;
    for byte in value.bytes().filter(|byte| *byte != b'=') {
        let upper = byte.to_ascii_uppercase();
        let digit = match upper {
            b'A'..=b'Z' => upper - b'A',
            b'2'..=b'7' => upper - b'2' + 26,
            _ => return None,
        } as u32;
        buffer = (buffer << 5) | digit;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            output.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    (!output.is_empty()).then_some(output)
}

type SessionResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug)]
struct SessionError(&'static str);

impl Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl Error for SessionError {}

fn kex_body(sub: u8, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(1 + payload.len());
    v.push(sub);
    v.extend_from_slice(payload);
    v
}

fn parse_kex(body: &[u8]) -> Option<(u8, &[u8])> {
    body.split_first().map(|(s, rest)| (*s, rest))
}

/// Live reliable session data handle. The payloads remain sealed; Task 3 owns
/// the protocol-level sealing/opening and file-transfer dispatch.
pub struct DataHandle {
    pub sender: transport::reliable::ReliableSender,
    pub receiver: mpsc::Receiver<Vec<u8>>,
    pub sealer: Sealer,
    pub opener: Opener,
    pub control_sender: transport::reliable::ReliableSender,
    /// The ONE control-direction sealer of this session. The session loop
    /// shares it (keepalives) so every control frame uses one nonce sequence;
    /// a second `Sealer` on the same key would reuse nonces.
    pub control_sealer: SharedSealer,
    /// The path media takes and its measured round trip, for display.
    pub link: transport::LinkStatus,
}

/// How often a viewer pings the host while media goes over the relay.
pub const RELAY_PING_INTERVAL: Duration = Duration::from_secs(1);

/// A sealer shared between the session loop and the application.
pub type SharedSealer = Arc<Mutex<Sealer>>;

/// How often each peer sends a sealed `Control::KeepAlive` on the relay
/// socket. Well below the relay's idle timeout (~60 s).
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
/// A peer that sent no control traffic (keepalives included) for this long is
/// considered gone and the session ends. Three missed keepalives.
pub const DEAD_PEER_TIMEOUT: Duration = Duration::from_secs(45);

/// Liveness timings of a session; tests shorten them.
#[derive(Debug, Clone, Copy)]
struct SessionTimings {
    keepalive: Duration,
    dead_peer: Duration,
}

impl Default for SessionTimings {
    fn default() -> Self { Self { keepalive: KEEPALIVE_INTERVAL, dead_peer: DEAD_PEER_TIMEOUT } }
}

fn seal_control(sealer: &SharedSealer, control: &Control) -> Vec<u8> {
    sealer.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).seal(&[Tag::Control as u8], &control.to_bytes())
}

/// Send a sealed `Control::KeepAlive` every `interval` until the writer
/// closes or the returned guard is dropped.
fn spawn_keepalive(sender: transport::reliable::ReliableSender, sealer: SharedSealer, interval: Duration) -> AbortOnDrop<()> {
    AbortOnDrop(tokio::spawn(async move {
        let mut ticks = tokio::time::interval_at(tokio::time::Instant::now() + interval, interval);
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticks.tick().await;
            let sealed = seal_control(&sealer, &Control::KeepAlive);
            if sender.send_frame(Tag::Control, &sealed).await.is_err() { break; }
        }
    }))
}

/// Best-effort sealed `Control::Bye` ending the session for the peer, bounded
/// (~200 ms) like [`send_rejection`] so a stalled writer cannot hold up the
/// caller. The short sleep lets the writer flush before it is torn down.
pub async fn send_bye(sender: &transport::reliable::ReliableSender, sealer: &SharedSealer) {
    let sealed = seal_control(sealer, &Control::Bye { session: String::new() });
    let sent = tokio::time::timeout(Duration::from_millis(150), sender.send_frame(Tag::Control, &sealed)).await;
    if matches!(sent, Ok(Ok(()))) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewerSessionError {
    pub reason: DisconnectReason,
    pub message: String,
}

impl Display for ViewerSessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.message) }
}
impl Error for ViewerSessionError {}

fn viewer_session_error(message: String) -> ViewerSessionError {
    ViewerSessionError { reason: classify_viewer_error(&message), message }
}

impl DataHandle {
    pub async fn send_data(&self, sealed: &[u8]) -> Result<(), transport::TransportError> {
        self.sender.send_frame(Tag::Data, sealed).await
    }
}

async fn recv_control(rx: &mut mpsc::Receiver<Vec<u8>>) -> Option<Vec<u8>> { rx.recv().await }

/// Best-effort sealed `Control::Error` telling the viewer why the host
/// rejected it after the key exchange. Bounded (~200ms total) so a stalled
/// writer can never hold up the host; the short sleep lets the WebSocket
/// writer flush before the session (and its writer task) is dropped.
async fn send_rejection(sender: &transport::reliable::ReliableSender, sealer: &SharedSealer, code: &str) {
    let sealed = seal_control(sealer, &Control::Error { code: code.to_owned() });
    let sent = tokio::time::timeout(Duration::from_millis(150), sender.send_frame(Tag::Control, &sealed)).await;
    if matches!(sent, Ok(Ok(()))) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn split_session_control(mut rx: mpsc::Receiver<Vec<u8>>, session: String) -> (mpsc::Receiver<Vec<u8>>, mpsc::Receiver<Vec<u8>>) {
    let (ice_tx, ice_rx) = mpsc::channel(64);
    let (session_tx, session_rx) = mpsc::channel(64);
    tokio::spawn(async move {
        while let Some(body) = rx.recv().await {
            let is_ice = matches!(Control::from_bytes(&body), Some(Control::Ice { session: body_session, .. }) if body_session == session);
            // Late ICE candidates after signaling is dropped are discarded;
            // only a closed session receiver ends the demux (which the host
            // loop observes as the peer being gone).
            if is_ice { let _ = ice_tx.send(body).await; } else if session_tx.send(body).await.is_err() { break; }
        }
    });
    (ice_rx, session_rx)
}

fn even_frame(frame: encode::RawFrame) -> Option<encode::RawFrame> {
    let width = frame.width & !1;
    let height = frame.height & !1;
    if width == 0
        || height == 0
        || frame.data.len() != usize::from(frame.width) * usize::from(frame.height) * 4
    {
        return None;
    }
    if width == frame.width && height == frame.height {
        return Some(frame);
    }
    let source_width = usize::from(frame.width);
    let cropped_width = usize::from(width);
    let mut data = Vec::with_capacity(cropped_width * usize::from(height) * 4);
    for row in frame
        .data
        .chunks_exact(source_width * 4)
        .take(usize::from(height))
    {
        data.extend_from_slice(&row[..cropped_width * 4]);
    }
    Some(encode::RawFrame {
        width,
        height,
        data,
        format: frame.format,
        timestamp_us: frame.timestamp_us,
    })
}

/// Run a host session after relay pairing.
///
/// The encoder factory receives the dimensions of the first usable captured
/// frame. The session performs SPAKE2 confirmation before starting either
/// capture or input injection, then runs both directions concurrently.
pub async fn run_host_session<S, C, E, F, I>(
    ws: WebSocketStream<S>,
    password: &str,
    ice_session: &str,
    capturer: C,
    encoder_factory: F,
    injector: I,
) -> SessionResult<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    C: ScreenCapturer + Send + 'static,
    E: Encoder + Send + 'static,
    F: Fn(u16, u16) -> Result<E, encode::EncoderInitError> + Send + 'static,
    I: InputInjector + Send + 'static,
{
    run_host_session_with_data(ws, password, ice_session, capturer, encoder_factory, injector, None).await
}

pub async fn run_host_session_with_data<S, C, E, F, I>(
    ws: WebSocketStream<S>, password: &str, ice_session: &str, capturer: C,
    encoder_factory: F, injector: I, data_ready: Option<oneshot::Sender<DataHandle>>,
) -> SessionResult<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    C: ScreenCapturer + Send + 'static,
    E: Encoder + Send + 'static,
    F: Fn(u16, u16) -> Result<E, encode::EncoderInitError> + Send + 'static,
    I: InputInjector + Send + 'static,
{
    run_host_session_with_data_and_accept(
        ws, password, ice_session, capturer, encoder_factory, injector, false,
        || async { true }, data_ready,
    ).await
}

/// Host session variant with an approval callback invoked only after SPAKE2
/// and key confirmation succeed. Unattended mode can return true without
/// presenting a manual approval request; the default path awaits the callback.
#[allow(clippy::too_many_arguments)]
pub async fn run_host_session_with_data_and_accept<S, C, E, F, I, A, Fut>(
    ws: WebSocketStream<S>, password: &str, ice_session: &str, capturer: C,
    encoder_factory: F, injector: I, unattended_enabled: bool, approve: A,
    data_ready: Option<oneshot::Sender<DataHandle>>,
) -> SessionResult<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    C: ScreenCapturer + Send + 'static,
    E: Encoder + Send + 'static,
    F: Fn(u16, u16) -> Result<E, encode::EncoderInitError> + Send + 'static,
    I: InputInjector + Send + 'static,
    A: FnOnce() -> Fut,
    Fut: Future<Output = bool>,
{
    run_host_session_with_data_and_accept_and_2fa(
        ws, password, ice_session, capturer, encoder_factory, injector,
        unattended_enabled, false, None, approve, data_ready,
    ).await
}

#[allow(clippy::too_many_arguments)]
pub async fn run_host_session_with_data_and_accept_and_2fa<S, C, E, F, I, A, Fut>(
    ws: WebSocketStream<S>, password: &str, ice_session: &str, capturer: C,
    encoder_factory: F, injector: I, unattended_enabled: bool,
    twofa_enabled: bool, totp_secret: Option<&str>, approve: A,
    data_ready: Option<oneshot::Sender<DataHandle>>,
) -> SessionResult<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    C: ScreenCapturer + Send + 'static,
    E: Encoder + Send + 'static,
    F: Fn(u16, u16) -> Result<E, encode::EncoderInitError> + Send + 'static,
    I: InputInjector + Send + 'static,
    A: FnOnce() -> Fut,
    Fut: Future<Output = bool>,
{
    run_host_session_with_data_and_accept_and_2fa_with_factory(
        ws, password, ice_session, || Some(capturer), encoder_factory,
        move || Box::new(injector) as Box<dyn InputInjector>,
        unattended_enabled, twofa_enabled, totp_secret, approve, data_ready, SessionTimings::default(),
    ).await
}

#[allow(clippy::too_many_arguments)]
async fn run_host_session_with_data_and_accept_and_2fa_with_factory<S, C, E, F, IF, A, Fut, CF>(
    ws: WebSocketStream<S>, password: &str, ice_session: &str, capture_factory: CF,
    encoder_factory: F, injector_factory: IF, unattended_enabled: bool,
    twofa_enabled: bool, totp_secret: Option<&str>, approve: A,
    data_ready: Option<oneshot::Sender<DataHandle>>, timings: SessionTimings,
)-> SessionResult<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    C: ScreenCapturer + Send + 'static,
    E: Encoder + Send + 'static,
    F: Fn(u16, u16) -> Result<E, encode::EncoderInitError> + Send + 'static,
    IF: FnOnce() -> Box<dyn InputInjector> + Send + 'static,
    A: FnOnce() -> Fut,
    Fut: Future<Output = bool>,
    CF: FnOnce() -> Option<C>,
{
    let mut reliable = ReliableChannel::spawn(ws);
    let sender = reliable.sender();
    let mut control_rx = reliable.take_control_rx().ok_or(SessionError("control receiver unavailable"))?;
    let data_rx = reliable.take_data_rx().ok_or(SessionError("data receiver unavailable"))?;
    let (state, my_msg) = protocol::pake::start(password.as_bytes(), true);
    sender.send_frame(Tag::Kex, &kex_body(KEX_MSG, &my_msg)).await?;
    let kex_deadline = tokio::time::Instant::now() + KEX_DEADLINE;
    let shared = loop {
        match tokio::time::timeout_at(kex_deadline, recv_control(&mut control_rx)).await.ok().flatten() {
            Some(body) => if let Some((KEX_MSG, msg)) = parse_kex(&body) {
                break protocol::pake::finish(state, msg).ok_or(SessionError("invalid SPAKE2 message"))?;
            },
            None => return Err(Box::new(SessionError("connection closed during KEX"))),
        }
    };
    let my_confirm = protocol::pake::confirm_tag(&shared, true);
    sender.send_frame(Tag::Kex, &kex_body(KEX_CONFIRM, &my_confirm)).await?;
    loop {
        match tokio::time::timeout_at(kex_deadline, recv_control(&mut control_rx)).await.ok().flatten() {
            Some(body) => if let Some((KEX_CONFIRM, tag)) = parse_kex(&body) {
                if !protocol::pake::verify_peer(&shared, false, tag) { return Err(Box::new(SessionError("key confirmation failed"))); }
                break;
            },
            None => return Err(Box::new(SessionError("connection closed during KEX confirmation"))),
        }
    }
    let (control_send_key, control_recv_key) = protocol::kex::derive_control_keys(&shared, true);
    let control_sealer: SharedSealer = Arc::new(Mutex::new(Sealer::new(control_send_key)));
    let mut control_opener = Opener::new(control_recv_key);
    // Keepalives start as soon as the peer is authenticated, so neither the
    // relay nor the viewer's dead-peer timer drops a session that is waiting
    // on the host user's approval.
    let _keepalive = spawn_keepalive(sender.clone(), Arc::clone(&control_sealer), timings.keepalive);
    let mut code_received = false;
    let code_valid = if unattended_enabled && twofa_enabled {
        let body = tokio::time::timeout(Duration::from_secs(30), recv_control(&mut control_rx)).await.ok().flatten();
        let code = body.and_then(|body| control_opener.open(&[Tag::Control as u8], &body)).and_then(|body| match Control::from_bytes(&body) {
            Some(Control::AuthCode { code }) if !code.is_empty() => Some(code),
            _ => None,
        });
        code_received = code.is_some();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |duration| duration.as_secs());
        code.as_deref().zip(totp_secret).is_some_and(|(code, secret)| totp_is_valid_once(secret, code, now))
    } else { true };
    let rejection = if unattended_enabled && twofa_enabled && !code_valid {
        Some(if code_received { reject_code::TWOFA_INVALID } else { reject_code::TWOFA_REQUIRED })
    } else if !should_auto_accept_with_2fa(unattended_enabled, true, twofa_enabled, code_valid) && !approve().await {
        Some(reject_code::DENIED)
    } else {
        None
    };
    if let Some(code) = rejection {
        // Tell the viewer explicitly why, so it stops retrying instead of
        // reading the close as a transient drop.
        send_rejection(&sender, &control_sealer, code).await;
        return Err(Box::new(SessionError("session rejected")));
    }
    let Some(capturer) = capture_factory() else {
        // Without this the viewer reads the close as a drop and reconnects.
        send_rejection(&sender, &control_sealer, reject_code::CAPTURE_UNAVAILABLE).await;
        return Err(Box::new(SessionError(capture_unavailable_message())));
    };
    reliable.enable_application_traffic();
    let (send_key, recv_key) = derive_keys(&shared, true);
    let sealer = Sealer::new(send_key);
    let opener = Opener::new(recv_key);
    // The Data channel uses a SEPARATE key (distinct HKDF label) from media, so
    // the two streams' independent nonce counters never collide under a shared
    // key. Sharing the media key here would be catastrophic AEAD nonce reuse.
    let (data_send_key, data_recv_key) = protocol::kex::derive_data_keys(&shared, true);
    let data_sealer = Sealer::new(data_send_key);
    let data_opener = Opener::new(data_recv_key);
    let (ice_rx, mut session_control_rx) = split_session_control(control_rx, ice_session.to_owned());

    let signaling: RelayIceSignaling<S> = RelayIceSignaling::new(sender.clone(), ice_rx, ice_session);
    let relay: RelayTransport<S> = RelayTransport::new(sender.clone(), reliable.take_media_rx().ok_or(SessionError("media receiver unavailable"))?);
    let transport = transport::establish(
        HOST_ICE_CONTROLLING,
        signaling,
        relay,
        Some(transport::FeedbackKeys::derive(&shared, true)),
        transport::default_agent_config(),
        Duration::from_secs(3),
    )
    .await;
    // Send the monitor list first, then share the SAME control sealer with the
    // data channel. Host->viewer control frames must share one sealer (one
    // nonce sequence) on this key: a second Sealer::new on control_send_key
    // would restart the counter at 0 and reuse a nonce with MonitorList.
    let monitors = capturer.monitors();
    let monitor_ids: Vec<u32> = monitors.iter().map(|monitor| monitor.id).collect();
    let monitor_list = Control::MonitorList { monitors };
    reliable.sender().send_frame(Tag::Control, &seal_control(&control_sealer, &monitor_list)).await?;
    let path = transport.path();
    let (transport_tx, transport_rx) = transport.into_split();
    if let Some(ready) = data_ready {
        let _ = ready.send(DataHandle {
            sender: reliable.sender(),
            receiver: data_rx,
            sealer: data_sealer,
            opener: data_opener,
            control_sender: reliable.sender(),
            control_sealer: Arc::clone(&control_sealer),
            link: transport::LinkStatus::new(path, transport_tx.rtt_meter()),
        });
    }
    let audio_rx = audio::spawn_capture();

    // Capture and encode run on their own thread, so a slow encode never
    // stalls control, keepalives, audio or input. It publishes the input
    // mapping dimensions once the first frame is in.
    let dims = Arc::new(AtomicU32::new(0));
    let rate = transport_tx.rate_control();
    let video = VideoSource::start(capturer, encoder_factory, Arc::clone(&dims), rate).await?;
    // Input injection makes blocking OS calls, so it runs on its own thread
    // and never stalls capture/encode/send or audio. The injector is built on
    // that thread because platform injectors (Enigo) can be thread-affine.
    let display = Arc::new(AtomicU64::new(pack_display(None)));
    let inject_tx = spawn_injection_thread(injector_factory, Arc::clone(&dims), Arc::clone(&display))?;
    // Aborting the forwarder on exit drops the last injection sender, which
    // ends the injection thread together with the session.
    let mut input_forwarder = AbortOnDrop(tokio::spawn(forward_input(transport_rx, opener, inject_tx)));
    let mut media = AbortOnDrop(tokio::spawn(send_media(transport_tx, sealer, audio_rx, Arc::clone(&video.units))));
    // The viewer keepalives on the relay socket even when media is direct, so
    // silence there means the viewer is gone (a direct transport may never
    // report a dead peer on its own).
    let mut peer_deadline = tokio::time::Instant::now() + timings.dead_peer;
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(peer_deadline) => {
                log::warn!("viewer sent no control traffic for {}s; ending the session", timings.dead_peer.as_secs());
                return Ok(());
            },
            body = session_control_rx.recv() => {
                // The reliable (WebSocket) channel closed: the peer is gone.
                let Some(body) = body else { return Ok(()) };
                let Some(plaintext) = control_opener.open(&[Tag::Control as u8], &body) else { continue };
                peer_deadline = tokio::time::Instant::now() + timings.dead_peer;
                match Control::from_bytes(&plaintext) {
                    // Authenticated by the per-session control key, so the
                    // session field is informational only.
                    Some(Control::Bye { .. }) => return Ok(()),
                    // The new monitor's frames arrive later; the video
                    // pipeline adapts to their size.
                    Some(Control::SelectMonitor { id }) => {
                        // Input follows the switch; the capturer ignores ids
                        // it did not advertise, so input does too.
                        if monitor_ids.contains(&id) {
                            display.store(pack_display(Some(id)), Ordering::Release);
                        }
                        video.send(VideoCommand::SelectMonitor(id));
                    }
                    Some(Control::RequestKeyframe) => video.send(VideoCommand::RequestKeyframe),
                    Some(Control::Ping { sent_us }) => {
                        let _ = sender.send_frame(Tag::Control, &seal_control(&control_sealer, &Control::Pong { sent_us })).await;
                    }
                    _ => {}
                }
            },
            ended = &mut input_forwarder.0 => {
                return match ended {
                    Ok(InputForwardEnd::PeerGone) => Ok(()),
                    Ok(InputForwardEnd::InjectorGone) => Err(Box::new(SessionError("input injection thread stopped"))),
                    Err(_) => Err(Box::new(SessionError("input forwarding task failed"))),
                };
            },
            ended = &mut media.0 => {
                return match ended {
                    Ok(result) => result,
                    Err(_) => Err(Box::new(SessionError("media sending task failed"))),
                };
            },
        }
    }
}

/// Sends audio and encoded video to the viewer until the transport fails or
/// the video pipeline stops. Audio goes first; a video frame is only taken
/// from the encoder once the transport has room for it, so on a slow link
/// the encoder skips captures rather than queueing stale frames.
async fn send_media(
    mut transport_tx: Box<dyn TransportSender>,
    mut sealer: Sealer,
    mut audio_rx: mpsc::Receiver<protocol::audio::AudioFrame>,
    units: Arc<UnitSlot>,
) -> SessionResult<()> {
    let mut video_ready = false;
    loop {
        tokio::select! {
            biased;
            Some(audio_frame) = audio_rx.recv() => {
                let sealed = sealer.seal(&[Tag::Audio as u8], &audio_frame.to_bytes());
                let _ = transport_tx.send(Tag::Audio, &sealed).await;
            },
            () = transport_tx.ready_for_video(), if !video_ready => video_ready = true,
            unit = units.take(), if video_ready => {
                let plaintext = unit?;
                video_ready = false;
                let sealed = sealer.seal(&[Tag::Video as u8], &plaintext);
                transport_tx.send(Tag::Video, &sealed).await?;
            },
        }
    }
}

/// How long a new session waits for the first captured frame.
const FIRST_FRAME_DEADLINE: Duration = Duration::from_secs(10);
/// Longest the encode thread blocks before it re-checks commands and
/// shutdown.
const VIDEO_POLL: Duration = Duration::from_millis(20);

/// Validates and even-crops a captured frame, then scales it to the stream
/// size. Returns the (even) capture size, which maps input, and the frame
/// to encode.
fn stream_frame(frame: encode::RawFrame) -> Option<((u16, u16), encode::RawFrame)> {
    let frame = even_frame(frame)?;
    let capture = (frame.width, frame.height);
    let (width, height) = stream::stream_dims(frame.width, frame.height);
    if (width, height) == capture {
        return Some((capture, frame));
    }
    let data = stream::downscale_bgra(&frame.data, frame.width, frame.height, width, height)?;
    Some((capture, encode::RawFrame { width, height, data, format: frame.format, timestamp_us: frame.timestamp_us }))
}

/// Turns captured frames into sealed-ready video plaintext (header plus
/// access unit).
///
/// Frames larger than the stream size are scaled down. A frame of a new
/// stream size (display mode or monitor change) replaces the encoder, which
/// starts with a keyframe; the input mapping follows the capture size.
/// Keyframes are forced periodically and on (rate-limited) viewer request.
struct VideoPipeline<E, F> {
    encoder_factory: F,
    encoder: E,
    /// The last frame, already at stream size; repeated on a static screen.
    frame: encode::RawFrame,
    capture: (u16, u16),
    dims: Arc<AtomicU32>,
    seq: u64,
    started: Instant,
    keyframes: KeyframePolicy,
    /// Follows the transport's bitrate estimate (none in tests).
    bitrate: Option<stream::BitrateFollower>,
}

impl<E, F> VideoPipeline<E, F>
where
    E: Encoder,
    F: Fn(u16, u16) -> Result<E, encode::EncoderInitError>,
{
    fn new(frame: encode::RawFrame, encoder_factory: F, dims: Arc<AtomicU32>) -> SessionResult<Self> {
        let (capture, frame) = stream_frame(frame)
            .ok_or(SessionError("screen capture returned an invalid or unusable frame"))?;
        let encoder = encoder_factory(frame.width, frame.height)?;
        dims.store(pack_dims(capture.0, capture.1), Ordering::Release);
        let now = Instant::now();
        Ok(Self { encoder_factory, encoder, frame, capture, dims, seq: 0, started: now, keyframes: KeyframePolicy::new(now), bitrate: None })
    }

    fn with_rate(mut self, rate: Arc<transport::RateControl>) -> Self {
        self.bitrate = Some(stream::BitrateFollower::new(rate));
        self
    }

    /// A viewer asked for a keyframe; returns whether it was accepted.
    fn request_keyframe(&mut self) -> bool { self.keyframes.request(Instant::now()) }

    fn keyframe_due(&self) -> bool { self.keyframes.due(Instant::now()) }

    /// Encodes `fresh` if it is a usable frame, else the previous frame.
    /// Returns `None` when the encoder produced no output.
    fn next(&mut self, fresh: Option<encode::RawFrame>) -> SessionResult<Option<Vec<u8>>> {
        if let Some((capture, frame)) = fresh.and_then(stream_frame) {
            if (frame.width, frame.height) != (self.frame.width, self.frame.height) {
                log::info!("stream size changed to {}x{} (capture {}x{})", frame.width, frame.height, capture.0, capture.1);
                self.encoder = (self.encoder_factory)(frame.width, frame.height)?;
                if let Some(bitrate) = &mut self.bitrate { bitrate.encoder_replaced(); }
            }
            if capture != self.capture {
                self.dims.store(pack_dims(capture.0, capture.1), Ordering::Release);
                self.capture = capture;
            }
            self.frame = frame;
        }
        let now = Instant::now();
        if self.keyframes.due(now) {
            self.encoder.force_intra_frame();
        }
        if let Some(bitrate) = &mut self.bitrate { bitrate.update(&mut self.encoder, self.frame.width, self.frame.height); }
        let Some(encoded) = self.encoder.encode(&self.frame) else { return Ok(None) };
        if encoded.keyframe {
            self.keyframes.keyframe_sent(now);
        }
        // Stamped at encode time: repeated frames need their own timestamps.
        // The header field is 32-bit microseconds and wraps after ~71 minutes.
        let timestamp_us = self.started.elapsed().as_micros() as u32;
        let mut plaintext = encode::video_header(
            self.seq, timestamp_us, encoded.keyframe, self.frame.width, self.frame.height,
        );
        plaintext.extend_from_slice(&encoded.data);
        self.seq += 1;
        Ok(Some(plaintext))
    }
}

/// Requests from the session loop to the encode thread.
enum VideoCommand {
    SelectMonitor(u32),
    RequestKeyframe,
}

/// Async-side handle of the capture/encode thread. Dropping it stops the
/// thread (which then drops the capturer and encoder) within one poll.
struct VideoSource {
    commands: std::sync::mpsc::Sender<VideoCommand>,
    units: Arc<UnitSlot>,
}

impl VideoSource {
    /// Starts the encode thread and waits until the first frame was captured
    /// and the encoder created (or either failed).
    async fn start<C, E, F>(capturer: C, encoder_factory: F, dims: Arc<AtomicU32>, rate: Arc<transport::RateControl>) -> SessionResult<Self>
    where
        C: ScreenCapturer + Send + 'static,
        E: Encoder + 'static,
        F: Fn(u16, u16) -> Result<E, encode::EncoderInitError> + Send + 'static,
    {
        let (commands, command_rx) = std::sync::mpsc::channel();
        let units = Arc::new(UnitSlot::default());
        let (ready_tx, ready_rx) = oneshot::channel();
        let thread_units = Arc::clone(&units);
        std::thread::Builder::new()
            .name("opendesk-video-encode".into())
            .spawn(move || {
                let _close = CloseOnExit(&thread_units);
                run_video_thread(capturer, encoder_factory, dims, rate, &command_rx, &thread_units, ready_tx);
            })?;
        match ready_rx.await {
            Ok(Ok(())) => Ok(Self { commands, units }),
            Ok(Err(message)) => Err(message.into()),
            Err(_) => Err(Box::new(SessionError("video encode thread stopped"))),
        }
    }

    fn send(&self, command: VideoCommand) { let _ = self.commands.send(command); }
}

impl Drop for VideoSource {
    fn drop(&mut self) { self.units.close(); }
}

/// Closes the slot when the encode thread ends, even by panic, so the
/// media task notices.
struct CloseOnExit<'a>(&'a UnitSlot);

impl Drop for CloseOnExit<'_> {
    fn drop(&mut self) { self.0.close(); }
}

/// Body of the encode thread: encode each new capture as soon as it arrives
/// and the transport has taken the previous access unit; on a static screen
/// repeat the last frame slowly.
fn run_video_thread<C, E, F>(
    mut capturer: C,
    encoder_factory: F,
    dims: Arc<AtomicU32>,
    rate: Arc<transport::RateControl>,
    commands: &std::sync::mpsc::Receiver<VideoCommand>,
    units: &UnitSlot,
    ready: oneshot::Sender<Result<(), String>>,
) where
    C: ScreenCapturer,
    E: Encoder,
    F: Fn(u16, u16) -> Result<E, encode::EncoderInitError>,
{
    let deadline = Instant::now() + FIRST_FRAME_DEADLINE;
    let first = loop {
        if let Some(frame) = capturer.wait_frame(VIDEO_POLL) {
            break frame;
        }
        if Instant::now() >= deadline {
            let _ = ready.send(Err("screen capture produced no frames".into()));
            return;
        }
        if ready.is_closed() {
            return;
        }
    };
    let mut video = match VideoPipeline::new(first, encoder_factory, dims).map(|video| video.with_rate(rate)) {
        Ok(video) => video,
        Err(error) => {
            let _ = ready.send(Err(error.to_string()));
            return;
        }
    };
    if ready.send(Ok(())).is_err() {
        return;
    }
    let mut last_encode: Option<Instant> = None;
    loop {
        loop {
            match commands.try_recv() {
                Ok(VideoCommand::SelectMonitor(id)) => { capturer.select_monitor(id); }
                Ok(VideoCommand::RequestKeyframe) => {
                    if !video.request_keyframe() { log::debug!("keyframe request ignored: one was just sent"); }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
            }
        }
        // Encode only once the previous access unit is on its way: while the
        // link is busy the capturer keeps just the newest frame.
        match units.wait_space(VIDEO_POLL) {
            SlotSpace::Free => {}
            SlotSpace::Full => continue,
            SlotSpace::Closed => return,
        }
        let wait = stream::capture_wait(last_encode.map(|at| at.elapsed()), VIDEO_POLL);
        let fresh = if wait.is_zero() { capturer.next_frame() } else { capturer.wait_frame(wait) };
        if !stream::encode_due(fresh.is_some(), video.keyframe_due(), last_encode.map(|at| at.elapsed())) {
            continue;
        }
        match video.next(fresh) {
            Ok(Some(unit)) => units.put(unit),
            Ok(None) => {}
            Err(error) => {
                units.fail(error.to_string());
                return;
            }
        }
        last_encode = Some(Instant::now());
    }
}

/// One-unit handoff from the encode thread to the media task.
#[derive(Default)]
struct UnitSlot {
    state: Mutex<SlotState>,
    /// Wakes the encode thread when the unit is taken or the slot closes.
    space: std::sync::Condvar,
    /// Wakes the media task when a unit arrives or the slot closes.
    filled: tokio::sync::Notify,
}

#[derive(Default)]
struct SlotState {
    unit: Option<Vec<u8>>,
    closed: bool,
    error: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
enum SlotSpace {
    Free,
    Full,
    Closed,
}

impl UnitSlot {
    fn lock(&self) -> std::sync::MutexGuard<'_, SlotState> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Waits up to `timeout` for the slot to be empty.
    fn wait_space(&self, timeout: Duration) -> SlotSpace {
        let (state, _) = self
            .space
            .wait_timeout_while(self.lock(), timeout, |state| state.unit.is_some() && !state.closed)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.closed { SlotSpace::Closed } else if state.unit.is_some() { SlotSpace::Full } else { SlotSpace::Free }
    }

    fn put(&self, unit: Vec<u8>) {
        self.lock().unit = Some(unit);
        self.filled.notify_one();
    }

    /// Closes the slot with an error for the media task.
    fn fail(&self, error: String) {
        self.lock().error.get_or_insert(error);
        self.close();
    }

    fn close(&self) {
        self.lock().closed = true;
        self.filled.notify_one();
        self.space.notify_all();
    }

    /// The next unit; an error once the slot closed. Cancel safe.
    async fn take(&self) -> SessionResult<Vec<u8>> {
        loop {
            let filled = self.filled.notified();
            {
                let mut state = self.lock();
                if let Some(unit) = state.unit.take() {
                    drop(state);
                    self.space.notify_all();
                    return Ok(unit);
                }
                if state.closed {
                    return Err(state.error.clone().unwrap_or_else(|| "video pipeline stopped".into()).into());
                }
            }
            filled.await;
        }
    }
}

/// Bounded queue between the input forwarder and the injection thread.
const INJECT_QUEUE: usize = 64;

fn pack_dims(width: u16, height: u16) -> u32 { (u32::from(width) << 16) | u32::from(height) }

fn unpack_dims(packed: u32) -> (u32, u32) { (packed >> 16, packed & 0xFFFF) }

/// The shared display as an atomic value: 0 is the main display, a selected
/// monitor id is tagged in the high half.
fn pack_display(display: Option<u32>) -> u64 { display.map_or(0, |id| (1 << 32) | u64::from(id)) }

fn unpack_display(packed: u64) -> Option<u32> { (packed >> 32 == 1).then_some(packed as u32) }

struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) { self.0.abort(); }
}

/// Start the dedicated input-injection thread. It builds the injector itself
/// (keeping thread-affine backends on one OS thread), reads the current
/// capture dimensions and shared display at injection time so a monitor
/// switch applies to the next event, and exits once every sender is dropped.
fn spawn_injection_thread<IF>(injector_factory: IF, dims: Arc<AtomicU32>, display: Arc<AtomicU64>) -> SessionResult<mpsc::Sender<InputEvent>>
where
    IF: FnOnce() -> Box<dyn InputInjector> + Send + 'static,
{
    let (tx, mut rx) = mpsc::channel::<InputEvent>(INJECT_QUEUE);
    std::thread::Builder::new()
        .name("opendesk-input-inject".into())
        .spawn(move || {
            let mut injector = injector_factory();
            let mut current = None;
            while let Some(event) = rx.blocking_recv() {
                let selected = unpack_display(display.load(Ordering::Acquire));
                if selected != current {
                    injector.select_display(selected);
                    current = selected;
                }
                let (width, height) = unpack_dims(dims.load(Ordering::Acquire));
                injector.inject(&event, width, height);
            }
        })?;
    Ok(tx)
}

#[derive(Debug, PartialEq, Eq)]
enum InputForwardEnd {
    /// The media transport closed: the viewer is gone.
    PeerGone,
    /// The injection thread stopped (its receiver was dropped).
    InjectorGone,
}

/// Open viewer input and queue it for the injection thread.
///
/// Drop policy when the injection queue is full: mouse moves are coalesced to
/// the single most recent pending position (absolute coordinates, so older
/// ones are redundant) and never block. Buttons, keys, chars and scrolls are
/// never dropped: the forwarder awaits queue capacity, which back-pressures
/// only this input path, never the media loop. A pending move is flushed
/// before any non-move event so clicks land at the latest pointer position.
async fn forward_input(
    mut transport_rx: Box<dyn TransportReceiver>,
    mut opener: Opener,
    inject_tx: mpsc::Sender<InputEvent>,
) -> InputForwardEnd {
    let mut pending_move: Option<InputEvent> = None;
    loop {
        tokio::select! {
            biased;
            permit = inject_tx.reserve(), if pending_move.is_some() => {
                let Ok(permit) = permit else { return InputForwardEnd::InjectorGone };
                if let Some(event) = pending_move.take() { permit.send(event); }
            }
            received = transport_rx.recv() => {
                let Some((tag, body)) = received else { return InputForwardEnd::PeerGone };
                if tag != Tag::Input { continue; }
                let Some(plaintext) = opener.open(&[Tag::Input as u8], &body) else { continue };
                let Some((event, _timestamp_us)) = InputEvent::from_bytes(&plaintext) else { continue };
                if matches!(event, InputEvent::MouseMove { .. }) {
                    if pending_move.is_some() {
                        pending_move = Some(event);
                        continue;
                    }
                    match inject_tx.try_send(event) {
                        Ok(()) => {}
                        Err(mpsc::error::TrySendError::Full(event)) => pending_move = Some(event),
                        Err(mpsc::error::TrySendError::Closed(_)) => return InputForwardEnd::InjectorGone,
                    }
                } else {
                    if let Some(pending) = pending_move.take() {
                        if inject_tx.send(pending).await.is_err() { return InputForwardEnd::InjectorGone; }
                    }
                    if inject_tx.send(event).await.is_err() { return InputForwardEnd::InjectorGone; }
                }
            }
        }
    }
}

/// Run the viewer side of a transport after SPAKE2 and key confirmation.
///
/// The viewer is the ICE-controlled peer and therefore uses the opposite
/// SPAKE2 role and directional keys from [`run_host_session`]. No media is
/// established or processed until peer confirmation has succeeded.
pub async fn run_viewer_session<S>(
    ws: WebSocketStream<S>,
    password: &str,
    ice_session: &str,
    frame_sink: tokio::sync::mpsc::Sender<Vec<u8>>,
    input_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
) -> Result<(), ViewerSessionError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    run_viewer_session_with_data(ws, password, ice_session, frame_sink, input_rx, None).await
}

pub async fn run_viewer_session_with_data<S>(
    ws: WebSocketStream<S>, password: &str, ice_session: &str,
    frame_sink: tokio::sync::mpsc::Sender<Vec<u8>>,
    input_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
    data_ready: Option<oneshot::Sender<DataHandle>>,
) -> Result<(), ViewerSessionError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    run_viewer_session_with_monitors(ws, password, ice_session, frame_sink, input_rx, data_ready, None, None).await
}

#[allow(clippy::too_many_arguments)]
pub async fn run_viewer_session_with_monitors<S>(
    ws: WebSocketStream<S>, password: &str, ice_session: &str,
    frame_sink: tokio::sync::mpsc::Sender<Vec<u8>>,
    input_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
    data_ready: Option<oneshot::Sender<DataHandle>>,
    monitor_sink: Option<tokio::sync::mpsc::Sender<Vec<u8>>>,
    audio_sink: Option<tokio::sync::mpsc::Sender<Vec<u8>>>,
) -> Result<(), ViewerSessionError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    run_viewer_session_with_monitors_and_auth_code(
        ws, password, ice_session, frame_sink, input_rx, data_ready,
        monitor_sink, audio_sink, None,
    ).await
}

#[allow(clippy::too_many_arguments)]
pub async fn run_viewer_session_with_monitors_and_auth_code<S>(
    ws: WebSocketStream<S>, password: &str, ice_session: &str,
    frame_sink: tokio::sync::mpsc::Sender<Vec<u8>>,
    input_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
    data_ready: Option<oneshot::Sender<DataHandle>>,
    monitor_sink: Option<tokio::sync::mpsc::Sender<Vec<u8>>>,
    audio_sink: Option<tokio::sync::mpsc::Sender<Vec<u8>>>,
    auth_code: Option<String>,
) -> Result<(), ViewerSessionError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let result = run_viewer_session_inner(ws, password, ice_session, frame_sink, input_rx, data_ready, monitor_sink, audio_sink, auth_code, SessionTimings::default()).await;
    result.map_err(viewer_session_error)
}

#[allow(clippy::too_many_arguments)]
async fn run_viewer_session_inner<S>(
    ws: WebSocketStream<S>, password: &str, ice_session: &str,
    frame_sink: tokio::sync::mpsc::Sender<Vec<u8>>,
    input_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
    data_ready: Option<oneshot::Sender<DataHandle>>,
    monitor_sink: Option<tokio::sync::mpsc::Sender<Vec<u8>>>,
    audio_sink: Option<tokio::sync::mpsc::Sender<Vec<u8>>>,
    auth_code: Option<String>,
    timings: SessionTimings,
) -> Result<(), String>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let mut reliable = ReliableChannel::spawn(ws);
    let sender = reliable.sender();
    let mut control_rx = reliable.take_control_rx().ok_or_else(|| "control receiver unavailable".to_owned())?;
    let data_rx = reliable.take_data_rx().ok_or_else(|| "data receiver unavailable".to_owned())?;
    let (state, my_msg) = protocol::pake::start(password.as_bytes(), false);
    sender.send_frame(Tag::Kex, &kex_body(KEX_MSG, &my_msg)).await.map_err(|error| error.to_string())?;
    let kex_deadline = tokio::time::Instant::now() + KEX_DEADLINE;

    let shared = loop {
        match tokio::time::timeout_at(kex_deadline, recv_control(&mut control_rx)).await.ok().flatten() {
            Some(body) => if let Some((KEX_MSG, msg)) = parse_kex(&body) {
                break protocol::pake::finish(state, msg).ok_or_else(|| "invalid SPAKE2 message".to_owned())?;
            },
            None => return Err("connection closed during KEX".to_owned()),
        }
    };

    let my_confirm = protocol::pake::confirm_tag(&shared, false);
    sender.send_frame(Tag::Kex, &kex_body(KEX_CONFIRM, &my_confirm)).await.map_err(|error| error.to_string())?;
    loop {
        match tokio::time::timeout_at(kex_deadline, recv_control(&mut control_rx)).await.ok().flatten() {
            Some(body) => if let Some((KEX_CONFIRM, tag)) = parse_kex(&body) {
                if !protocol::pake::verify_peer(&shared, true, tag) { return Err("key confirmation failed".to_owned()); }
                break;
            },
            None => return Err("connection closed during KEX confirmation".to_owned()),
        }
    }
    reliable.enable_application_traffic();

    let (send_key, recv_key) = derive_keys(&shared, false);
    let sealer = Sealer::new(send_key);
    let opener = Opener::new(recv_key);
    // Separate Data-channel key (see host session) — avoids nonce reuse.
    let (data_send_key, data_recv_key) = protocol::kex::derive_data_keys(&shared, false);
    let data_sealer = Sealer::new(data_send_key);
    let data_opener = Opener::new(data_recv_key);
    let (control_send_key, control_recv_key) = protocol::kex::derive_control_keys(&shared, false);
    let control_sealer: SharedSealer = Arc::new(Mutex::new(Sealer::new(control_send_key)));
    let mut control_opener = Opener::new(control_recv_key);
    // Always send one (empty = no code) so a 2FA host can answer at once
    // instead of waiting for its timeout; other hosts ignore it.
    let auth = Control::AuthCode { code: auth_code.unwrap_or_default() };
    sender.send_frame(Tag::Control, &seal_control(&control_sealer, &auth)).await.map_err(|error| error.to_string())?;
    // Started after AuthCode so a 2FA host reads the code as the first
    // control frame.
    let _keepalive = spawn_keepalive(sender.clone(), Arc::clone(&control_sealer), timings.keepalive);
    let (ice_rx, mut session_control_rx) = split_session_control(control_rx, ice_session.to_owned());
    // Open session control frames in one place (one opener, in order) and
    // split an explicit host rejection (`Control::Error`) out of the stream so
    // every phase below can react to it promptly.
    let (control_plain_tx, control_plain_rx) = mpsc::channel::<Vec<u8>>(64);
    let (rejected_tx, mut rejected_rx) = mpsc::channel::<String>(1);
    let _control_demux = AbortOnDrop(tokio::spawn(async move {
        while let Some(body) = session_control_rx.recv().await {
            let Some(plaintext) = control_opener.open(&[Tag::Control as u8], &body) else { continue };
            if let Some(Control::Error { code }) = Control::from_bytes(&plaintext) {
                let _ = rejected_tx.send(code).await;
                break;
            }
            if control_plain_tx.send(plaintext).await.is_err() { break; }
        }
    }));

    let signaling: RelayIceSignaling<S> = RelayIceSignaling::new(sender.clone(), ice_rx, ice_session);
    let relay: RelayTransport<S> = RelayTransport::new(sender.clone(), reliable.take_media_rx().ok_or_else(|| "media receiver unavailable".to_owned())?);
    let establish = transport::establish(
        VIEWER_ICE_CONTROLLING,
        signaling,
        relay,
        Some(transport::FeedbackKeys::derive(&shared, false)),
        transport::default_agent_config(),
        Duration::from_secs(3),
    );
    let transport = tokio::select! {
        biased;
        Some(code) = rejected_rx.recv() => return Err(rejection_error(&code)),
        transport = establish => transport,
    };
    let path = transport.path();
    let (transport_tx, transport_rx) = transport.into_split();
    if let Some(ready) = data_ready {
        let _ = ready.send(DataHandle {
            sender: reliable.sender(),
            receiver: data_rx,
            sealer: data_sealer,
            opener: data_opener,
            control_sender: reliable.sender(),
            control_sealer: Arc::clone(&control_sealer),
            link: transport::LinkStatus::new(path, transport_tx.rtt_meter()),
        });
    }

    // The relay carries no feedback, so the viewer measures its round trip
    // itself. A direct path reports RTT through UDP feedback instead.
    let relay_ping = (path == transport::MediaPath::Relay).then_some(RELAY_PING_INTERVAL);
    let host_control = HostControl { sender: reliable.sender(), sealer: Arc::clone(&control_sealer), relay_ping };
    run_viewer_media_loop_with_control(transport_tx, transport_rx, frame_sink, input_rx, sealer, opener, control_plain_rx, rejected_rx, monitor_sink, audio_sink, Some(host_control), timings.dead_peer).await
}

/// Ask the host for a keyframe, e.g. after a decoder error. Uses the
/// session's reliable control channel and control sealer (a viewer gets
/// both from its [`DataHandle`]: `control_sender` and `control_sealer`).
/// The host ignores requests within 250 ms of its last keyframe.
pub async fn request_keyframe(sender: &transport::reliable::ReliableSender, sealer: &SharedSealer) -> Result<(), transport::TransportError> {
    sender.send_frame(Tag::Control, &seal_control(sealer, &Control::RequestKeyframe)).await
}

/// What the viewer media loop needs to send control frames to the host:
/// keyframe requests, and pings while media goes over the relay.
struct HostControl {
    sender: transport::reliable::ReliableSender,
    sealer: SharedSealer,
    relay_ping: Option<Duration>,
}

/// Relay round trips from sealed `Ping`/`Pong` frames. The send time
/// travels in the ping and comes back in the pong, so a late pong still
/// gives a correct sample. A host from beta.5 or older never answers.
struct RelayPinger {
    clock: Instant,
}

impl RelayPinger {
    fn new(clock: Instant) -> Self { Self { clock } }

    fn micros(&self, now: Instant) -> u64 { now.saturating_duration_since(self.clock).as_micros() as u64 }

    fn ping(&self, now: Instant) -> Control { Control::Ping { sent_us: self.micros(now) } }

    /// The round trip of the ping a pong answers; `None` if it claims to be
    /// from the future.
    fn rtt(&self, sent_us: u64, now: Instant) -> Option<Duration> {
        self.micros(now).checked_sub(sent_us).map(Duration::from_micros)
    }
}

/// How long the viewer waits for a requested keyframe before asking again.
const KEYFRAME_RETRY: Duration = Duration::from_millis(500);

/// Viewer-side continuity check of the video sequence. A frame after a gap
/// (lost UDP fragments, a dropped frame) references data the decoder never
/// got, so frames are held back until the next keyframe and the host is
/// asked for one.
#[derive(Debug)]
struct VideoGate {
    next_seq: Option<u64>,
    awaiting_keyframe: bool,
    last_request: Option<Instant>,
}

impl VideoGate {
    fn new() -> Self { Self { next_seq: None, awaiting_keyframe: true, last_request: None } }

    /// Whether frame `seq` can be decoded.
    fn accept(&mut self, seq: u64, keyframe: bool) -> bool {
        if self.next_seq.is_some_and(|next| seq < next) {
            // Late or duplicate: the decoder has moved past it.
            return false;
        }
        if keyframe {
            self.awaiting_keyframe = false;
        } else if self.awaiting_keyframe || self.next_seq.is_some_and(|next| seq != next) {
            self.awaiting_keyframe = true;
            return false;
        }
        self.next_seq = Some(seq + 1);
        true
    }

    /// An accepted frame never reached the decoder.
    fn lost(&mut self) { self.awaiting_keyframe = true; }

    /// Whether to (re)send a keyframe request now.
    fn should_request(&mut self, now: Instant) -> bool {
        if !self.awaiting_keyframe || self.last_request.is_some_and(|at| now.duration_since(at) < KEYFRAME_RETRY) {
            return false;
        }
        self.last_request = Some(now);
        true
    }
}

/// (seq, keyframe) from a video plaintext header (see `encode::video_header`).
fn video_seq(plaintext: &[u8]) -> Option<(u64, bool)> {
    let header = plaintext.get(..18)?;
    Some((u64::from_be_bytes(header[..8].try_into().ok()?), header[13] & 1 == 1))
}

#[allow(dead_code)]
async fn run_viewer_media_loop(
    transport_tx: Box<dyn TransportSender>,
    transport_rx: Box<dyn TransportReceiver>,
    frame_sink: tokio::sync::mpsc::Sender<Vec<u8>>,
    input_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
    sealer: Sealer,
    opener: Opener,
) -> Result<(), String> {
    let (_tx, rx) = mpsc::channel(1);
    let (_rejected_tx, rejected_rx) = mpsc::channel(1);
    run_viewer_media_loop_with_control(transport_tx, transport_rx, frame_sink, input_rx, sealer, opener, rx, rejected_rx, None, None, None, DEAD_PEER_TIMEOUT).await
}

#[allow(clippy::too_many_arguments)]
async fn run_viewer_media_loop_with_control(
    mut transport_tx: Box<dyn TransportSender>,
    mut transport_rx: Box<dyn TransportReceiver>,
    frame_sink: tokio::sync::mpsc::Sender<Vec<u8>>,
    mut input_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
    mut sealer: Sealer,
    mut opener: Opener,
    mut control_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
    mut rejected_rx: tokio::sync::mpsc::Receiver<String>,
    monitor_sink: Option<tokio::sync::mpsc::Sender<Vec<u8>>>,
    audio_sink: Option<tokio::sync::mpsc::Sender<Vec<u8>>>,
    host_control: Option<HostControl>,
    dead_peer: Duration,
) -> Result<(), String> {
    let rtt = transport_tx.rtt_meter();
    let pinger = RelayPinger::new(Instant::now());
    let relay_ping = host_control.as_ref().and_then(|control| control.relay_ping);
    let mut ping_ticks = tokio::time::interval(relay_ping.unwrap_or(RELAY_PING_INTERVAL));
    ping_ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // The host keepalives on the relay socket even when media is direct, so
    // silence there means the host is gone: a direct transport never reports
    // a dead peer on its own.
    let mut peer_deadline = tokio::time::Instant::now() + dead_peer;
    let mut gate = VideoGate::new();
    // A decodable frame the frame sink had no room for yet. Video never
    // blocks this loop: input and control keep flowing while it waits.
    let mut pending_video: Option<Vec<u8>> = None;
    loop {
        tokio::select! {
            // Input is polled before media so a video burst never delays it.
            biased;
            Some(code) = rejected_rx.recv() => return Err(rejection_error(&code)),
            plaintext = control_rx.recv() => {
                let Some(plaintext) = plaintext else {
                    // The relay socket closed: the host is gone (or rejected
                    // us, which the demux reports just before closing).
                    if let Ok(code) = rejected_rx.try_recv() { return Err(rejection_error(&code)); }
                    return Err("connection closed: the host's control channel closed".into());
                };
                peer_deadline = tokio::time::Instant::now() + dead_peer;
                match Control::from_bytes(&plaintext) {
                    Some(Control::Bye { .. }) => return Err(crate::reconnect::HOST_ENDED_MESSAGE.into()),
                    Some(Control::MonitorList { .. }) => {
                        if let Some(sink) = &monitor_sink { let _ = sink.send(plaintext).await; }
                    }
                    Some(Control::Pong { sent_us }) if relay_ping.is_some() => {
                        let now = Instant::now();
                        if let Some(sample) = pinger.rtt(sent_us, now) { rtt.record(sample, now); }
                    }
                    _ => {}
                }
            },
            input = input_rx.recv() => {
                let Some(bytes) = input else { return Ok(()) };
                let sealed = sealer.seal(&[Tag::Input as u8], &bytes);
                if transport_tx.send(Tag::Input, &sealed).await.is_err() { return Err("viewer input transport closed".into()) }
            }
            _ = ping_ticks.tick(), if relay_ping.is_some() => {
                if let Some(control) = &host_control {
                    let ping = seal_control(&control.sealer, &pinger.ping(Instant::now()));
                    let _ = control.sender.send_frame(Tag::Control, &ping).await;
                }
            }
            _ = tokio::time::sleep_until(peer_deadline) => {
                return Err(format!("connection closed: host stopped responding for {}s", dead_peer.as_secs()));
            },
            permit = frame_sink.reserve(), if pending_video.is_some() => {
                let Ok(permit) = permit else { return Err("viewer frame channel closed".into()) };
                if let Some(frame) = pending_video.take() { permit.send(frame); }
            }
            received = transport_rx.recv() => {
                let Some((tag, sealed)) = received else {
                    // The host's rejection and its close can race through
                    // separate channels; prefer the explicit reason if it is
                    // already on its way.
                    if let Ok(Some(code)) = tokio::time::timeout(Duration::from_millis(300), rejected_rx.recv()).await {
                        return Err(rejection_error(&code));
                    }
                    return Err("connection closed during session".into())
                };
                let Some(plaintext) = opener.open(&[tag as u8], &sealed) else { continue };
                match tag {
                    Tag::Video => {
                        let Some((seq, keyframe)) = video_seq(&plaintext) else { continue };
                        if gate.accept(seq, keyframe) {
                            if pending_video.is_some() && !keyframe {
                                // The sink is still behind: this delta cannot
                                // be queued without breaking its reference.
                                gate.lost();
                            } else {
                                // A keyframe supersedes a stale pending frame.
                                pending_video = None;
                                match frame_sink.try_send(plaintext) {
                                    Ok(()) => {}
                                    Err(mpsc::error::TrySendError::Full(frame)) => pending_video = Some(frame),
                                    Err(mpsc::error::TrySendError::Closed(_)) => return Err("viewer frame channel closed".into()),
                                }
                            }
                        }
                        if gate.should_request(Instant::now()) {
                            if let Some(requests) = &host_control {
                                log::debug!("video frames lost; requesting a keyframe");
                                let _ = request_keyframe(&requests.sender, &requests.sealer).await;
                            }
                        }
                    }
                    // Audio is dropped rather than waited for.
                    Tag::Audio => if let Some(sink) = &audio_sink {
                        if let Err(mpsc::error::TrySendError::Closed(_)) = sink.try_send(plaintext) { return Err("viewer audio channel closed".into()) }
                    },
                    _ => {}
                }
            }
        }
    }
}

fn capture_unavailable_message() -> &'static str {
    #[cfg(target_os = "macos")]
    return "Screen capture is unavailable. Grant this app macOS Screen Recording permission and try again.";

    #[cfg(target_os = "windows")]
    return "Screen capture is unavailable. Windows Graphics Capture could not be started.";

    #[cfg(target_os = "linux")]
    return "Screen capture is unavailable. Approve the Linux desktop portal capture request and try again.";
}

#[cfg(target_os = "macos")]
fn report_input_unavailable() {
    log::warn!("input injection unavailable: macOS Accessibility permission is required; grant it in System Settings (x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility), then restart the host");
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn report_input_unavailable() {
    // Enigo is backed by SendInput on Windows and XTest/uinput on Linux.
    log::warn!("input injection unavailable: the platform input backend could not be initialized");
}

/// Build the production injector. Called on the injection thread, so the
/// thread-affine Enigo handle is created and used on one OS thread.
fn default_injector() -> Box<dyn InputInjector> {
    match EnigoInjector::new() {
        Some(injector) => Box::new(injector),
        None => {
            report_input_unavailable();
            Box::new(NoopInjector::default())
        }
    }
}

/// The production encoder for a stream of `width` x `height`, at the stream
/// frame rate with a bitrate scaled to the stream size.
fn default_encoder(width: u16, height: u16) -> Result<VideoEncoder, encode::EncoderInitError> {
    VideoEncoder::best_available(width, height, STREAM_FPS, stream::stream_bitrate(width, height, STREAM_FPS))
}

/// Run a host session with the production capture, H.264 encoder, and Enigo injector.
pub async fn run_default_host_session<S>(
    ws: WebSocketStream<S>,
    password: &str,
    ice_session: &str,
) -> SessionResult<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    run_host_session_with_data_and_accept_and_2fa_with_factory(
        ws, password, ice_session, ScapCapturer::new,
        default_encoder,
        default_injector, false, false, None, || async { true }, None, SessionTimings::default(),
    )
    .await
}

pub async fn run_default_host_session_with_data<S>(
    ws: WebSocketStream<S>,
    password: &str,
    ice_session: &str,
    data_ready: oneshot::Sender<DataHandle>,
) -> SessionResult<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    run_default_host_session_with_data_and_accept(
        ws, password, ice_session, false, || async { true }, data_ready,
    ).await
}

pub async fn run_default_host_session_with_data_and_accept<S, A, Fut>(
    ws: WebSocketStream<S>,
    password: &str,
    ice_session: &str,
    unattended_enabled: bool,
    approve: A,
    data_ready: oneshot::Sender<DataHandle>,
) -> SessionResult<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    A: FnOnce() -> Fut,
    Fut: Future<Output = bool>,
{
    run_default_host_session_with_data_and_accept_and_2fa(
        ws, password, ice_session, unattended_enabled, false, None, approve, data_ready,
    ).await
}

#[allow(clippy::too_many_arguments)]
pub async fn run_default_host_session_with_data_and_accept_and_2fa<S, A, Fut>(
    ws: WebSocketStream<S>, password: &str, ice_session: &str,
    unattended_enabled: bool, twofa_enabled: bool, totp_secret: Option<&str>,
    approve: A, data_ready: oneshot::Sender<DataHandle>,
) -> SessionResult<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    A: FnOnce() -> Fut,
    Fut: Future<Output = bool>,
{
    run_host_session_with_data_and_accept_and_2fa_with_factory(
        ws,
        password,
        ice_session,
        ScapCapturer::new,
        default_encoder,
        default_injector,
        unattended_enabled,
        twofa_enabled,
        totp_secret,
        approve,
        Some(data_ready),
        SessionTimings::default(),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::DirectTransport;
    use protocol::kex::derive_keys;
    use std::sync::Arc;
    use tokio::time::{timeout, Duration};
    use webrtc_util::conn::conn_pipe::pipe;

    #[test]
    fn host_and_viewer_ice_roles_are_opposite() {
        // Compile-time invariant: host and viewer MUST take opposite ICE
        // controlling roles, or ICE never nominates a candidate pair.
        const _: () = assert!(HOST_ICE_CONTROLLING != VIEWER_ICE_CONTROLLING);
    }

    #[test]
    fn unattended_auto_accept_requires_successful_pake() {
        assert!(should_auto_accept(true, true));
        assert!(!should_auto_accept(true, false));
        assert!(!should_auto_accept(false, true));
        assert!(!should_auto_accept(false, false));
    }

    #[test]
    fn allow_list_auto_accept_depends_only_on_membership() {
        assert!(should_auto_accept_allowlisted(true));
        assert!(!should_auto_accept_allowlisted(false));
    }

    #[test]
    fn totp_accepts_current_and_adjacent_steps_but_not_wrong_or_expired_codes() {
        // A real authenticator base32-decodes the enrolled secret to get the
        // HMAC key, so the expected codes must be computed over the DECODED key.
        let secret = "JBSWY3DPEHPK3PXP"; // valid RFC 4648 base32
        let key = decode_base32(secret).expect("valid base32 secret");
        let now = 1_700_000_000;
        let current = totp_lite::totp_custom::<totp_lite::Sha1>(30, 6, &key, now);
        let previous = totp_lite::totp_custom::<totp_lite::Sha1>(30, 6, &key, now - 30);
        let next = totp_lite::totp_custom::<totp_lite::Sha1>(30, 6, &key, now + 30);
        let expired = totp_lite::totp_custom::<totp_lite::Sha1>(30, 6, &key, now - 60);
        assert!(totp_is_valid(secret, &current, now));
        assert!(totp_is_valid(secret, &previous, now));
        assert!(totp_is_valid(secret, &next, now));
        assert!(!totp_is_valid(secret, "000000", now));
        assert!(!totp_is_valid(secret, &expired, now));
    }

    /// Known-answer test: RFC 6238 Appendix B SHA-1 vectors (truncated to 6
    /// digits) for the ASCII seed "12345678901234567890", base32-encoded.
    /// Also pins SHA-256 (FIPS 180-2 "abc"), used for file digests and the
    /// TOTP replay-cache key.
    #[test]
    fn totp_and_sha256_match_known_answers() {
        let secret = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
        for (time, code) in [
            (59, "287082"),
            (1_111_111_109, "081804"),
            (1_111_111_111, "050471"),
            (1_234_567_890, "005924"),
            (2_000_000_000, "279037"),
        ] {
            assert_eq!(totp_lite::totp_custom::<totp_lite::Sha1>(30, 6, &decode_base32(secret).unwrap(), time), code);
            assert!(totp_is_valid(secret, code, time));
        }
        let digest: [u8; 32] = Sha256::digest(b"abc").into();
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn totp_replay_guard_consumes_a_step_but_accepts_a_fresh_step() {
        let secret = "GEZDGNBVGY3TQOJQ";
        let key = decode_base32(secret).unwrap();
        let now = 1_700_000_000;
        let current = totp_lite::totp_custom::<totp_lite::Sha1>(30, 6, &key, now);
        let fresh = totp_lite::totp_custom::<totp_lite::Sha1>(30, 6, &key, now + 30);
        assert!(totp_is_valid_once(secret, &current, now));
        assert!(!totp_is_valid_once(secret, &current, now));
        assert!(totp_is_valid_once(secret, &fresh, now + 30));
    }

    #[test]
    fn twofa_gate_requires_a_valid_code() {
        assert!(should_auto_accept_with_2fa(true, true, false, false));
        assert!(should_auto_accept_with_2fa(true, true, true, true));
        assert!(!should_auto_accept_with_2fa(true, true, true, false));
        assert!(!should_auto_accept_with_2fa(false, true, true, true));
    }

    #[tokio::test]
    async fn viewer_media_loop_opens_video_and_seals_input() {
        let shared = [23u8; 32];
        let (viewer_send, viewer_recv) = derive_keys(&shared, false);
        let (host_send, host_recv) = derive_keys(&shared, true);
        let mut host_video_sealer = Sealer::new(host_send);
        let mut host_input_opener = Opener::new(host_recv);

        let (viewer_conn, host_conn) = pipe();
        let (viewer_tx, viewer_rx) = DirectTransport::new(Arc::new(viewer_conn)).into_split();
        let (mut host_tx, mut host_rx) = DirectTransport::new(Arc::new(host_conn)).into_split();
        let (frame_sink, mut frames) = tokio::sync::mpsc::channel(1);
        let (input_tx, input_rx) = tokio::sync::mpsc::channel(1);

        let viewer = tokio::spawn(run_viewer_media_loop(
            viewer_tx,
            viewer_rx,
            frame_sink,
            input_rx,
            Sealer::new(viewer_send),
            Opener::new(viewer_recv),
        ));

        let mut video = encode::video_header(0, 0, true, 64, 48);
        video.extend_from_slice(b"h264");
        let sealed_video = host_video_sealer.seal(&[Tag::Video as u8], &video);
        host_tx.send(Tag::Video, &sealed_video).await.unwrap();
        assert_eq!(timeout(Duration::from_secs(1), frames.recv()).await.unwrap(), Some(video));

        let input = b"encoded input event".to_vec();
        input_tx.send(input.clone()).await.unwrap();
        let (tag, sealed_input) = timeout(Duration::from_secs(1), host_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(tag, Tag::Input);
        assert_eq!(host_input_opener.open(&[Tag::Input as u8], &sealed_input), Some(input));

        drop(input_tx);
        assert!(timeout(Duration::from_secs(1), viewer).await.unwrap().unwrap().is_ok());
    }

    struct ChannelReceiver(mpsc::Receiver<(Tag, Vec<u8>)>);

    #[async_trait::async_trait]
    impl TransportReceiver for ChannelReceiver {
        async fn recv(&mut self) -> Option<(Tag, Vec<u8>)> { self.0.recv().await }
    }

    #[tokio::test]
    async fn forward_input_coalesces_moves_but_keeps_keys_in_order() {
        let shared = [5u8; 32];
        let (viewer_send, _) = derive_keys(&shared, false);
        let (_, host_recv) = derive_keys(&shared, true);
        let mut viewer_sealer = Sealer::new(viewer_send);
        let (wire_tx, wire_rx) = mpsc::channel(16);
        // Capacity 1 and no consumer yet: the queue is full after one event.
        let (inject_tx, mut inject_rx) = mpsc::channel(1);
        let forwarder = tokio::spawn(forward_input(Box::new(ChannelReceiver(wire_rx)), Opener::new(host_recv), inject_tx));

        let key = |code| InputEvent::Key { code, down: true, modifiers: 0 };
        let mv = |x| InputEvent::MouseMove { x, y: x };
        for event in [key(65), mv(1), mv(2), mv(3), key(66)] {
            let sealed = viewer_sealer.seal(&[Tag::Input as u8], &event.to_bytes(0));
            wire_tx.send((Tag::Input, sealed)).await.unwrap();
        }
        let mut received = Vec::new();
        for _ in 0..3 {
            received.push(timeout(Duration::from_secs(1), inject_rx.recv()).await.unwrap().unwrap());
        }
        // Stale moves collapse to the latest one, flushed before the next key.
        assert_eq!(received, vec![key(65), mv(3), key(66)]);

        drop(wire_tx);
        assert_eq!(timeout(Duration::from_secs(1), forwarder).await.unwrap().unwrap(), InputForwardEnd::PeerGone);
    }

    #[tokio::test]
    async fn forward_input_reports_a_stopped_injection_thread() {
        let (wire_tx, wire_rx) = mpsc::channel(4);
        let (inject_tx, inject_rx) = mpsc::channel(1);
        drop(inject_rx);
        let shared = [6u8; 32];
        let (viewer_send, _) = derive_keys(&shared, false);
        let (_, host_recv) = derive_keys(&shared, true);
        let forwarder = tokio::spawn(forward_input(Box::new(ChannelReceiver(wire_rx)), Opener::new(host_recv), inject_tx));
        let event = InputEvent::Key { code: 65, down: true, modifiers: 0 };
        let sealed = Sealer::new(viewer_send).seal(&[Tag::Input as u8], &event.to_bytes(0));
        wire_tx.send((Tag::Input, sealed)).await.unwrap();
        assert_eq!(timeout(Duration::from_secs(1), forwarder).await.unwrap().unwrap(), InputForwardEnd::InjectorGone);
    }

    #[test]
    fn injection_thread_uses_latest_dims_and_exits_when_senders_drop() {
        struct Probe(std::sync::mpsc::Sender<(u32, u32)>);
        impl InputInjector for Probe {
            fn inject(&mut self, _ev: &InputEvent, w: u32, h: u32) { let _ = self.0.send((w, h)); }
        }
        let (seen_tx, seen_rx) = std::sync::mpsc::channel();
        let dims = Arc::new(AtomicU32::new(pack_dims(640, 480)));
        let display = Arc::new(AtomicU64::new(pack_display(None)));
        let tx = spawn_injection_thread(move || Box::new(Probe(seen_tx)) as Box<dyn InputInjector>, Arc::clone(&dims), display).unwrap();
        let event = InputEvent::MouseButton { button: 1, down: true };
        tx.blocking_send(event).unwrap();
        assert_eq!(seen_rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap(), (640, 480));
        dims.store(pack_dims(1920, 1080), Ordering::Release);
        tx.blocking_send(event).unwrap();
        assert_eq!(seen_rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap(), (1920, 1080));
        drop(tx);
        // The injector (and its probe sender) drops when the thread exits.
        assert_eq!(seen_rx.recv_timeout(std::time::Duration::from_secs(1)), Err(std::sync::mpsc::RecvTimeoutError::Disconnected));
    }

    #[test]
    fn injection_thread_tells_the_injector_the_selected_display() {
        struct Probe(std::sync::mpsc::Sender<Option<u32>>);
        impl InputInjector for Probe {
            fn inject(&mut self, _ev: &InputEvent, _w: u32, _h: u32) {}
            fn select_display(&mut self, display: Option<u32>) { let _ = self.0.send(display); }
        }
        let (seen_tx, seen_rx) = std::sync::mpsc::channel();
        let dims = Arc::new(AtomicU32::new(pack_dims(640, 480)));
        let display = Arc::new(AtomicU64::new(pack_display(None)));
        let tx = spawn_injection_thread(move || Box::new(Probe(seen_tx)) as Box<dyn InputInjector>, dims, Arc::clone(&display)).unwrap();
        let event = InputEvent::MouseMove { x: 0, y: 0 };
        let wait = std::time::Duration::from_secs(1);
        // The main display is the injector's default: nothing to report yet.
        tx.blocking_send(event).unwrap();
        display.store(pack_display(Some(0)), Ordering::Release);
        tx.blocking_send(event).unwrap();
        tx.blocking_send(event).unwrap();
        assert_eq!(seen_rx.recv_timeout(wait).unwrap(), Some(0));
        display.store(pack_display(Some(u32::MAX)), Ordering::Release);
        tx.blocking_send(event).unwrap();
        assert_eq!(seen_rx.recv_timeout(wait).unwrap(), Some(u32::MAX));
        drop(tx);
        // Reported once per change, then the thread exits.
        assert_eq!(seen_rx.recv_timeout(wait), Err(std::sync::mpsc::RecvTimeoutError::Disconnected));
    }

    /// (seq, keyframe, width, height) from a video plaintext header.
    fn video_header_fields(plaintext: &[u8]) -> (u64, bool, u16, u16) {
        (
            u64::from_be_bytes(plaintext[0..8].try_into().unwrap()),
            plaintext[13] & 1 == 1,
            u16::from_be_bytes([plaintext[14], plaintext[15]]),
            u16::from_be_bytes([plaintext[16], plaintext[17]]),
        )
    }

    #[test]
    fn video_pipeline_follows_capture_size_changes_and_repeats_static_frames() {
        use crate::capture::{FakeCapturer, ScreenCapturer};
        let mut capturer = FakeCapturer::new(64, 48);
        let dims = Arc::new(AtomicU32::new(0));
        let first = even_frame(capturer.next_frame().unwrap()).unwrap();
        let mut video = VideoPipeline::new(
            first,
            |w, h| encode::H264Encoder::new(w, h, 15, 1_000_000),
            Arc::clone(&dims),
        ).unwrap();
        assert_eq!(unpack_dims(dims.load(Ordering::Acquire)), (64, 48));

        let first = video.next(None).unwrap().expect("first frame encodes");
        assert_eq!(video_header_fields(&first), (0, true, 64, 48));
        // No new capture: the last frame is repeated, not skipped.
        let repeat = video.next(None).unwrap().expect("repeated frame encodes");
        assert_eq!(video_header_fields(&repeat), (1, false, 64, 48));

        // The display changes size (an odd width is cropped to even).
        capturer.set_size(97, 32);
        let resized = video.next(capturer.next_frame()).unwrap().expect("resized frame encodes");
        assert_eq!(video_header_fields(&resized), (2, true, 96, 32));
        assert_eq!(unpack_dims(dims.load(Ordering::Acquire)), (96, 32));
        let repeat = video.next(None).unwrap().expect("repeated frame encodes");
        assert_eq!(video_header_fields(&repeat), (3, false, 96, 32));
    }

    #[test]
    fn video_pipeline_turns_encoder_init_failure_into_an_error() {
        use crate::capture::{FakeCapturer, ScreenCapturer};
        // Only 64x48 encoders can be created.
        let factory = |w, h| {
            if (w, h) == (64, 48) {
                encode::H264Encoder::new(w, h, 15, 1_000_000)
            } else {
                Err(encode::EncoderInitError::new("unsupported size"))
            }
        };
        let mut capturer = FakeCapturer::new(32, 32);
        let dims = Arc::new(AtomicU32::new(0));
        let small = even_frame(capturer.next_frame().unwrap()).unwrap();
        assert!(VideoPipeline::new(small, factory, Arc::clone(&dims)).is_err());

        capturer.set_size(64, 48);
        let frame = even_frame(capturer.next_frame().unwrap()).unwrap();
        let mut video = VideoPipeline::new(frame, factory, dims).unwrap();
        capturer.set_size(32, 32);
        assert!(video.next(capturer.next_frame()).is_err());
    }

    #[test]
    fn video_pipeline_scales_large_captures_but_maps_input_to_the_capture() {
        use crate::capture::{FakeCapturer, ScreenCapturer};
        let mut capturer = FakeCapturer::new(3840, 2160);
        let dims = Arc::new(AtomicU32::new(0));
        let mut video = VideoPipeline::new(
            capturer.next_frame().unwrap(),
            |w, h| encode::H264Encoder::new(w, h, STREAM_FPS, stream::stream_bitrate(w, h, STREAM_FPS)),
            Arc::clone(&dims),
        ).unwrap();
        let first = video.next(None).unwrap().expect("encodes");
        assert_eq!(video_header_fields(&first), (0, true, 1920, 1080));
        assert_eq!(unpack_dims(dims.load(Ordering::Acquire)), (3840, 2160));

        // A smaller capture is streamed as is.
        capturer.set_size(1280, 720);
        let small = video.next(capturer.next_frame()).unwrap().expect("encodes");
        assert_eq!(video_header_fields(&small), (1, true, 1280, 720));
        assert_eq!(unpack_dims(dims.load(Ordering::Acquire)), (1280, 720));
    }

    #[test]
    fn video_pipeline_honours_keyframe_requests_with_a_rate_limit() {
        use crate::capture::{FakeCapturer, ScreenCapturer};
        let mut capturer = FakeCapturer::new(64, 48);
        let mut video = VideoPipeline::new(
            capturer.next_frame().unwrap(),
            |w, h| encode::H264Encoder::new(w, h, STREAM_FPS, 1_000_000),
            Arc::new(AtomicU32::new(0)),
        ).unwrap();
        assert!(video_header_fields(&video.next(None).unwrap().unwrap()).1);
        // Too soon after the first keyframe: ignored.
        assert!(!video.request_keyframe());
        assert!(!video_header_fields(&video.next(None).unwrap().unwrap()).1);
        std::thread::sleep(stream::MIN_REQUESTED_KEYFRAME_SPACING);
        assert!(video.request_keyframe());
        assert!(video.keyframe_due());
        assert!(video_header_fields(&video.next(None).unwrap().unwrap()).1, "requested keyframe");
        assert!(!video.keyframe_due());
        assert!(!video_header_fields(&video.next(None).unwrap().unwrap()).1);
    }

    #[test]
    fn video_gate_drops_frames_after_a_gap_until_a_keyframe() {
        let mut gate = VideoGate::new();
        let now = Instant::now();
        // A stream must start at a keyframe.
        assert!(!gate.accept(0, false));
        assert!(gate.should_request(now));
        assert!(gate.accept(1, true));
        assert!(gate.accept(2, false));
        assert!(!gate.should_request(now + KEYFRAME_RETRY));
        // Frame 3 is lost: 4 and 5 reference it.
        assert!(!gate.accept(4, false));
        assert!(gate.should_request(now + KEYFRAME_RETRY));
        assert!(!gate.accept(5, false));
        assert!(!gate.should_request(now + KEYFRAME_RETRY), "requests are rate limited");
        assert!(gate.should_request(now + KEYFRAME_RETRY * 2), "an unanswered request is repeated");
        // A late frame 3 does not resume the stream either.
        assert!(!gate.accept(3, false));
        assert!(gate.accept(6, true));
        assert!(gate.accept(7, false));
        // Duplicates and stale frames are ignored without a request.
        assert!(!gate.accept(7, false));
        assert!(!gate.accept(1, true));
        assert!(!gate.should_request(now + KEYFRAME_RETRY * 3));
    }

    #[test]
    fn video_gate_waits_for_a_keyframe_after_a_local_drop() {
        let mut gate = VideoGate::new();
        assert!(gate.accept(0, true));
        gate.lost();
        assert!(!gate.accept(1, false));
        assert!(gate.should_request(Instant::now()));
        assert!(gate.accept(2, true));
    }

    #[test]
    fn video_seq_reads_the_header() {
        let mut plaintext = encode::video_header(42, 7, true, 64, 48);
        plaintext.extend_from_slice(b"au");
        assert_eq!(video_seq(&plaintext), Some((42, true)));
        assert_eq!(video_seq(&encode::video_header(43, 7, false, 64, 48)), Some((43, false)));
        assert_eq!(video_seq(&[0; 17]), None);
    }

    #[tokio::test]
    async fn unit_slot_hands_over_one_unit_at_a_time() {
        let slot = Arc::new(UnitSlot::default());
        assert_eq!(slot.wait_space(Duration::ZERO), SlotSpace::Free);
        slot.put(vec![1]);
        assert_eq!(slot.wait_space(Duration::from_millis(5)), SlotSpace::Full);
        assert_eq!(slot.take().await.unwrap(), vec![1]);
        assert_eq!(slot.wait_space(Duration::ZERO), SlotSpace::Free);

        // A waiting producer wakes as soon as the unit is taken.
        slot.put(vec![2]);
        let producer = {
            let slot = Arc::clone(&slot);
            std::thread::spawn(move || slot.wait_space(Duration::from_secs(5)))
        };
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(slot.take().await.unwrap(), vec![2]);
        assert_eq!(producer.join().unwrap(), SlotSpace::Free);

        slot.fail("encoder broke".into());
        assert_eq!(slot.wait_space(Duration::ZERO), SlotSpace::Closed);
        assert_eq!(slot.take().await.unwrap_err().to_string(), "encoder broke");
    }

    #[tokio::test]
    async fn unit_slot_take_waits_for_a_unit() {
        let slot = Arc::new(UnitSlot::default());
        let consumer = tokio::spawn({
            let slot = Arc::clone(&slot);
            async move { slot.take().await.unwrap() }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        slot.put(vec![9]);
        assert_eq!(timeout(Duration::from_secs(1), consumer).await.unwrap().unwrap(), vec![9]);
    }

    #[tokio::test]
    async fn viewer_requests_a_keyframe_after_a_sequence_gap() {
        use futures_util::StreamExt;
        use tokio_tungstenite::tungstenite::protocol::Role;
        let shared = [31u8; 32];
        let (viewer_send, viewer_recv) = derive_keys(&shared, false);
        let (host_send, _) = derive_keys(&shared, true);
        let mut host_video_sealer = Sealer::new(host_send);
        let (viewer_control_send, _) = protocol::kex::derive_control_keys(&shared, false);
        let (_, host_control_recv) = protocol::kex::derive_control_keys(&shared, true);

        // The viewer's control channel, read raw on the host end.
        let (viewer_ws, host_ws) = tokio::io::duplex(64 * 1024);
        let viewer_ws = WebSocketStream::from_raw_socket(viewer_ws, Role::Client, None).await;
        let mut host_ws = WebSocketStream::from_raw_socket(host_ws, Role::Server, None).await;
        let reliable = ReliableChannel::spawn(viewer_ws);
        let requests = HostControl { sender: reliable.sender(), sealer: Arc::new(Mutex::new(Sealer::new(viewer_control_send))), relay_ping: None };

        let (viewer_conn, host_conn) = pipe();
        let (viewer_tx, viewer_rx) = DirectTransport::new(Arc::new(viewer_conn)).into_split();
        let (mut host_tx, _host_rx) = DirectTransport::new(Arc::new(host_conn)).into_split();
        let (frame_sink, mut frames) = mpsc::channel(8);
        let (_input_tx, input_rx) = mpsc::channel(1);
        let (_control_tx, control_rx) = mpsc::channel(1);
        let (_rejected_tx, rejected_rx) = mpsc::channel(1);
        let viewer = tokio::spawn(run_viewer_media_loop_with_control(
            viewer_tx, viewer_rx, frame_sink, input_rx, Sealer::new(viewer_send), Opener::new(viewer_recv),
            control_rx, rejected_rx, None, None, Some(requests), DEAD_PEER_TIMEOUT,
        ));

        for (seq, keyframe) in [(0, true), (1, false), (3, false), (4, true)] {
            let plaintext = encode::video_header(seq, 0, keyframe, 64, 48);
            let sealed = host_video_sealer.seal(&[Tag::Video as u8], &plaintext);
            host_tx.send(Tag::Video, &sealed).await.unwrap();
        }
        let mut seqs = Vec::new();
        for _ in 0..3 {
            let frame = timeout(Duration::from_secs(1), frames.recv()).await.unwrap().unwrap();
            seqs.push(video_seq(&frame).unwrap().0);
        }
        assert_eq!(seqs, [0, 1, 4], "frame 3 follows a gap and is held back");

        let message = timeout(Duration::from_secs(1), host_ws.next()).await.unwrap().unwrap().unwrap();
        let (tag, body) = protocol::frame::unwrap(&message.into_data()).map(|(tag, body)| (tag, body.to_vec())).unwrap();
        assert_eq!(tag, Tag::Control);
        let plaintext = Opener::new(host_control_recv).open(&[Tag::Control as u8], &body).unwrap();
        assert_eq!(Control::from_bytes(&plaintext), Some(Control::RequestKeyframe));
        viewer.abort();
    }

    /// A viewer media loop whose control frames are read raw on the host
    /// end. Returns the loop, the viewer's RTT meter, a way to feed it host
    /// control plaintext, and a reader for the control frames it sends.
    struct PingRig {
        viewer: tokio::task::JoinHandle<Result<(), String>>,
        rtt: Arc<transport::RttMeter>,
        host_control: mpsc::Sender<Vec<u8>>,
        host_ws: WebSocketStream<tokio::io::DuplexStream>,
        host_opener: Opener,
        _input: mpsc::Sender<Vec<u8>>,
        _rejected: mpsc::Sender<String>,
        _reliable: ReliableChannel<tokio::io::DuplexStream>,
        // Dropping the far end would end the loop at once.
        _host_conn: Arc<dyn webrtc_util::conn::Conn + Send + Sync>,
    }

    impl PingRig {
        async fn start(relay_ping: Option<Duration>) -> Self {
            use tokio_tungstenite::tungstenite::protocol::Role;
            let shared = [33u8; 32];
            let (viewer_send, viewer_recv) = derive_keys(&shared, false);
            let (viewer_control_send, _) = protocol::kex::derive_control_keys(&shared, false);
            let (_, host_control_recv) = protocol::kex::derive_control_keys(&shared, true);
            let (viewer_ws, host_ws) = tokio::io::duplex(64 * 1024);
            let viewer_ws = WebSocketStream::from_raw_socket(viewer_ws, Role::Client, None).await;
            let host_ws = WebSocketStream::from_raw_socket(host_ws, Role::Server, None).await;
            let reliable = ReliableChannel::spawn(viewer_ws);
            let control = HostControl { sender: reliable.sender(), sealer: Arc::new(Mutex::new(Sealer::new(viewer_control_send))), relay_ping };
            let (viewer_conn, host_conn) = pipe();
            let (viewer_tx, viewer_rx) = DirectTransport::new(Arc::new(viewer_conn)).into_split();
            let rtt = viewer_tx.rtt_meter();
            let (frame_sink, _frames) = mpsc::channel(1);
            let (input, input_rx) = mpsc::channel(1);
            let (host_control, control_rx) = mpsc::channel(4);
            let (rejected, rejected_rx) = mpsc::channel(1);
            let viewer = tokio::spawn(run_viewer_media_loop_with_control(
                viewer_tx, viewer_rx, frame_sink, input_rx, Sealer::new(viewer_send), Opener::new(viewer_recv),
                control_rx, rejected_rx, None, None, Some(control), DEAD_PEER_TIMEOUT,
            ));
            Self { viewer, rtt, host_control, host_ws, host_opener: Opener::new(host_control_recv), _input: input, _rejected: rejected, _reliable: reliable, _host_conn: Arc::new(host_conn) }
        }

        /// The next control frame the viewer sent, if one comes soon.
        async fn next_control(&mut self) -> Option<Control> {
            use futures_util::StreamExt;
            let message = timeout(Duration::from_millis(300), self.host_ws.next()).await.ok()??.ok()?;
            let (tag, body) = protocol::frame::unwrap(&message.into_data()).map(|(tag, body)| (tag, body.to_vec()))?;
            assert_eq!(tag, Tag::Control);
            Control::from_bytes(&self.host_opener.open(&[Tag::Control as u8], &body)?)
        }
    }

    #[tokio::test]
    async fn viewer_measures_the_relay_rtt_with_ping_and_pong() {
        let mut rig = PingRig::start(Some(Duration::from_millis(50))).await;
        assert_eq!(rig.rtt.latest(Instant::now()), None);
        let Some(Control::Ping { sent_us }) = rig.next_control().await else { panic!("expected a ping") };
        tokio::time::sleep(Duration::from_millis(20)).await;
        rig.host_control.send(Control::Pong { sent_us }.to_bytes()).await.unwrap();
        let rtt = timeout(Duration::from_secs(1), async {
            loop {
                if let Some(rtt) = rig.rtt.latest(Instant::now()) { break rtt; }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("a pong gives an RTT");
        assert!(rtt >= Duration::from_millis(20) && rtt < Duration::from_secs(1), "{rtt:?}");
        rig.viewer.abort();
    }

    #[tokio::test]
    async fn an_old_host_that_never_answers_leaves_the_rtt_unknown() {
        let mut rig = PingRig::start(Some(Duration::from_millis(50))).await;
        for _ in 0..3 {
            assert!(matches!(rig.next_control().await, Some(Control::Ping { .. })));
        }
        // A beta.5 host keeps the session going (keepalives) but never pongs.
        rig.host_control.send(Control::KeepAlive.to_bytes()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(rig.rtt.latest(Instant::now()), None);
        assert!(!rig.viewer.is_finished());
        rig.viewer.abort();
    }

    #[tokio::test]
    async fn viewer_does_not_ping_when_media_is_direct() {
        let mut rig = PingRig::start(None).await;
        assert_eq!(rig.next_control().await, None);
        // A stray pong is not about the media path.
        rig.host_control.send(Control::Pong { sent_us: 0 }.to_bytes()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(rig.rtt.latest(Instant::now()), None);
        rig.viewer.abort();
    }

    #[test]
    fn relay_pinger_rejects_pongs_from_the_future() {
        let start = Instant::now();
        let pinger = RelayPinger::new(start);
        let Control::Ping { sent_us } = pinger.ping(start + Duration::from_millis(5)) else { unreachable!() };
        assert_eq!(pinger.rtt(sent_us, start + Duration::from_millis(35)), Some(Duration::from_millis(30)));
        assert_eq!(pinger.rtt(sent_us, start), None);
    }

    #[tokio::test]
    async fn viewer_input_flows_while_the_frame_sink_is_full() {
        let shared = [32u8; 32];
        let (viewer_send, viewer_recv) = derive_keys(&shared, false);
        let (host_send, host_recv) = derive_keys(&shared, true);
        let mut host_video_sealer = Sealer::new(host_send);
        let mut host_input_opener = Opener::new(host_recv);
        let (viewer_conn, host_conn) = pipe();
        let (viewer_tx, viewer_rx) = DirectTransport::new(Arc::new(viewer_conn)).into_split();
        let (mut host_tx, mut host_rx) = DirectTransport::new(Arc::new(host_conn)).into_split();
        // Nobody drains the frame sink.
        let (frame_sink, _frames) = mpsc::channel(1);
        let (input_tx, input_rx) = mpsc::channel(4);
        let viewer = tokio::spawn(run_viewer_media_loop(
            viewer_tx, viewer_rx, frame_sink, input_rx, Sealer::new(viewer_send), Opener::new(viewer_recv),
        ));
        for seq in 0..10 {
            let sealed = host_video_sealer.seal(&[Tag::Video as u8], &encode::video_header(seq, 0, seq == 0, 64, 48));
            host_tx.send(Tag::Video, &sealed).await.unwrap();
        }
        input_tx.send(b"click".to_vec()).await.unwrap();
        let (tag, sealed) = timeout(Duration::from_secs(1), host_rx.recv()).await.expect("input is not blocked by video").unwrap();
        assert_eq!(tag, Tag::Input);
        assert_eq!(host_input_opener.open(&[Tag::Input as u8], &sealed), Some(b"click".to_vec()));
        viewer.abort();
    }

    mod liveness {
        use super::*;
        use crate::{capture::FakeCapturer, encode::H264Encoder, inject::NoopInjector};
        use futures_util::{SinkExt, StreamExt};
        use std::sync::atomic::AtomicBool;
        use tokio::io::DuplexStream;
        use tokio::task::JoinHandle;
        use tokio_tungstenite::tungstenite::{protocol::Role, Message};

        const PASSWORD: &str = "liveness-pw";
        const SESSION: &str = "liveness";

        /// In-process stand-in for the relay: forwards Binary messages between
        /// the peers and, like the relay, drops both sockets once either side
        /// has sent nothing for `idle`.
        struct TestRelay {
            idle_fired: Arc<AtomicBool>,
            blackhole: Arc<AtomicBool>,
            task: JoinHandle<()>,
        }

        impl TestRelay {
            async fn start(idle: Duration) -> (Self, WebSocketStream<DuplexStream>, WebSocketStream<DuplexStream>) {
                Self::start_with(idle, true).await
            }

            /// With `ice` false the relay drops ICE candidates, so the session
            /// falls back to relayed media.
            async fn start_with(idle: Duration, ice: bool) -> (Self, WebSocketStream<DuplexStream>, WebSocketStream<DuplexStream>) {
                let (host_end, host_relay) = tokio::io::duplex(8 * 1024 * 1024);
                let (viewer_end, viewer_relay) = tokio::io::duplex(8 * 1024 * 1024);
                let host_ws = WebSocketStream::from_raw_socket(host_end, Role::Client, None).await;
                let viewer_ws = WebSocketStream::from_raw_socket(viewer_end, Role::Client, None).await;
                let (mut to_host, mut from_host) = WebSocketStream::from_raw_socket(host_relay, Role::Server, None).await.split();
                let (mut to_viewer, mut from_viewer) = WebSocketStream::from_raw_socket(viewer_relay, Role::Server, None).await.split();
                let idle_fired = Arc::new(AtomicBool::new(false));
                let blackhole = Arc::new(AtomicBool::new(false));
                let (task_idle, task_blackhole) = (Arc::clone(&idle_fired), Arc::clone(&blackhole));
                let task = tokio::spawn(async move {
                    let mut host_seen = tokio::time::Instant::now();
                    let mut viewer_seen = host_seen;
                    loop {
                        tokio::select! {
                            message = from_host.next() => {
                                host_seen = tokio::time::Instant::now();
                                let Some(Ok(message)) = message else { break };
                                if message.is_binary() && !task_blackhole.load(Ordering::SeqCst) && (ice || !is_ice(&message)) && to_viewer.send(message).await.is_err() { break; }
                            }
                            message = from_viewer.next() => {
                                viewer_seen = tokio::time::Instant::now();
                                let Some(Ok(message)) = message else { break };
                                if message.is_binary() && !task_blackhole.load(Ordering::SeqCst) && (ice || !is_ice(&message)) && to_host.send(message).await.is_err() { break; }
                            }
                            _ = tokio::time::sleep_until(host_seen.min(viewer_seen) + idle) => {
                                task_idle.store(true, Ordering::SeqCst);
                                break;
                            }
                        }
                    }
                    // Dropping the relay halves closes both peers' sockets.
                    let _ = to_host.send(Message::Close(None)).await;
                    let _ = to_viewer.send(Message::Close(None)).await;
                });
                (Self { idle_fired, blackhole, task }, host_ws, viewer_ws)
            }

            /// Close both sockets now, as the relay does when a peer leaves.
            fn close(&self) { self.task.abort(); }

            /// Keep both sockets open but stop forwarding: a silently dead peer.
            fn blackhole(&self) { self.blackhole.store(true, Ordering::SeqCst); }
        }

        /// ICE candidates are the only unsealed control frames in a session.
        fn is_ice(message: &Message) -> bool {
            let data = message.clone().into_data();
            matches!(protocol::frame::unwrap(&data), Some((Tag::Control, body)) if matches!(Control::from_bytes(body), Some(Control::Ice { .. })))
        }

        fn spawn_host(ws: WebSocketStream<DuplexStream>, timings: SessionTimings, ready: Option<oneshot::Sender<DataHandle>>) -> JoinHandle<SessionResult<()>> {
            tokio::spawn(async move {
                run_host_session_with_data_and_accept_and_2fa_with_factory(
                    ws, PASSWORD, SESSION, || Some(FakeCapturer::new(64, 64)),
                    |width, height| H264Encoder::new(width, height, 15, 1_000_000),
                    || Box::new(NoopInjector::default()) as Box<dyn InputInjector>,
                    false, false, None, || async { true }, ready, timings,
                ).await
            })
        }

        struct Viewer {
            task: JoinHandle<Result<(), ViewerSessionError>>,
            frames: mpsc::Receiver<Vec<u8>>,
            _input: mpsc::Sender<Vec<u8>>,
        }

        fn spawn_viewer(ws: WebSocketStream<DuplexStream>, timings: SessionTimings) -> Viewer {
            let (frame_tx, frames) = mpsc::channel(64);
            let (input, input_rx) = mpsc::channel(4);
            let task = tokio::spawn(async move {
                run_viewer_session_inner(ws, PASSWORD, SESSION, frame_tx, input_rx, None, None, None, None, timings)
                    .await
                    .map_err(viewer_session_error)
            });
            Viewer { task, frames, _input: input }
        }

        async fn first_frame(viewer: &mut Viewer) {
            timeout(Duration::from_secs(20), viewer.frames.recv())
                .await
                .expect("timed out waiting for the first video frame")
                .expect("viewer frame sink closed");
        }

        fn drain_frames(viewer: &mut Viewer) { while viewer.frames.try_recv().is_ok() {} }

        #[tokio::test(flavor = "multi_thread")]
        async fn viewer_keyframe_request_gets_a_keyframe() {
            let timings = SessionTimings::default();
            let (_relay, host_ws, viewer_ws) = TestRelay::start(Duration::from_secs(3600)).await;
            let host = spawn_host(host_ws, timings, None);
            let (frame_tx, mut frames) = mpsc::channel(64);
            let (_input, input_rx) = mpsc::channel(4);
            let (ready_tx, ready_rx) = oneshot::channel();
            let viewer = tokio::spawn(async move {
                run_viewer_session_inner(viewer_ws, PASSWORD, SESSION, frame_tx, input_rx, Some(ready_tx), None, None, None, timings).await
            });
            let handle = timeout(Duration::from_secs(20), ready_rx).await.unwrap().unwrap();
            let first = timeout(Duration::from_secs(20), frames.recv()).await.unwrap().unwrap();
            assert!(video_header_fields(&first).1, "the stream starts with a keyframe");

            // Past the host's rate limit, well before its periodic keyframe.
            tokio::time::sleep(stream::MIN_REQUESTED_KEYFRAME_SPACING + Duration::from_millis(50)).await;
            while frames.try_recv().is_ok() {}
            request_keyframe(&handle.control_sender, &handle.control_sealer).await.unwrap();
            timeout(Duration::from_secs(2), async {
                loop {
                    let frame = frames.recv().await.expect("viewer frame sink closed");
                    let (seq, keyframe, _, _) = video_header_fields(&frame);
                    if keyframe && seq > 0 { break; }
                }
            })
            .await
            .expect("the host answers a keyframe request");
            host.abort();
            viewer.abort();
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn relayed_viewer_measures_the_rtt_through_the_host() {
            let timings = SessionTimings::default();
            let (_relay, host_ws, viewer_ws) = TestRelay::start_with(Duration::from_secs(3600), false).await;
            let host = spawn_host(host_ws, timings, None);
            let (frame_tx, mut frames) = mpsc::channel(64);
            let (_input, input_rx) = mpsc::channel(4);
            let (ready_tx, ready_rx) = oneshot::channel();
            let viewer = tokio::spawn(async move {
                run_viewer_session_inner(viewer_ws, PASSWORD, SESSION, frame_tx, input_rx, Some(ready_tx), None, None, None, timings).await
            });
            let handle = timeout(Duration::from_secs(20), ready_rx).await.unwrap().unwrap();
            let rtt = timeout(Duration::from_secs(5), async {
                loop {
                    while frames.try_recv().is_ok() {}
                    if let Some(rtt) = handle.link.rtt(Instant::now()) { break rtt; }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .expect("the viewer measures an RTT");
            assert_eq!(handle.link.path, transport::MediaPath::Relay);
            assert!(rtt < Duration::from_secs(1), "{rtt:?}");
            host.abort();
            viewer.abort();
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn keepalive_holds_idle_relay_sockets_open() {
            let timings = SessionTimings { keepalive: Duration::from_millis(100), dead_peer: Duration::from_secs(5) };
            let (relay, host_ws, viewer_ws) = TestRelay::start(Duration::from_millis(600)).await;
            let host = spawn_host(host_ws, timings, None);
            let mut viewer = spawn_viewer(viewer_ws, timings);
            first_frame(&mut viewer).await;

            // Several idle timeouts: the viewer sends no input at all.
            for _ in 0..4 {
                tokio::time::sleep(Duration::from_millis(500)).await;
                drain_frames(&mut viewer);
            }
            assert!(!relay.idle_fired.load(Ordering::SeqCst), "keepalives must keep both relay sockets active");
            assert!(!host.is_finished(), "host session ended despite keepalives");
            assert!(!viewer.task.is_finished(), "viewer session ended despite keepalives");
            host.abort();
            viewer.task.abort();
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn without_keepalive_the_idle_relay_drops_the_session() {
            // Control experiment for the test above: same relay, keepalives off.
            let timings = SessionTimings { keepalive: Duration::from_secs(3600), dead_peer: Duration::from_secs(3600) };
            let (relay, host_ws, viewer_ws) = TestRelay::start(Duration::from_millis(600)).await;
            let host = spawn_host(host_ws, timings, None);
            let mut viewer = spawn_viewer(viewer_ws, timings);
            // Setup (ICE) alone can idle a direction past the timeout, so do
            // not wait for video: the session must simply be dropped.
            timeout(Duration::from_secs(10), async {
                while !relay.idle_fired.load(Ordering::SeqCst) {
                    drain_frames(&mut viewer);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            })
            .await
            .expect("the idle relay should drop a session without keepalives");
            host.abort();
            viewer.task.abort();
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn viewer_ends_promptly_when_host_control_channel_closes() {
            // Liveness timers far away: only the closed channel can end it.
            let timings = SessionTimings { keepalive: Duration::from_secs(3600), dead_peer: Duration::from_secs(3600) };
            let (relay, host_ws, viewer_ws) = TestRelay::start(Duration::from_secs(3600)).await;
            let host = spawn_host(host_ws, timings, None);
            let mut viewer = spawn_viewer(viewer_ws, timings);
            first_frame(&mut viewer).await;

            relay.close();
            let ended = timeout(Duration::from_secs(3), async {
                loop {
                    drain_frames(&mut viewer);
                    if viewer.task.is_finished() { break; }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await;
            assert!(ended.is_ok(), "viewer kept running after the host's control channel closed");
            let error = viewer.task.await.unwrap().expect_err("a vanished host is an error");
            assert_eq!(error.reason, DisconnectReason::Transient, "a dropped host must be reconnectable: {error}");
            let host_result = timeout(Duration::from_secs(3), host).await.expect("host kept running after the relay closed");
            assert!(host_result.unwrap().is_ok());
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn silent_peer_times_out_on_both_sides() {
            let timings = SessionTimings { keepalive: Duration::from_millis(100), dead_peer: Duration::from_millis(800) };
            let (relay, host_ws, viewer_ws) = TestRelay::start(Duration::from_secs(3600)).await;
            let host = spawn_host(host_ws, timings, None);
            let mut viewer = spawn_viewer(viewer_ws, timings);
            first_frame(&mut viewer).await;

            relay.blackhole();
            let ended = timeout(Duration::from_secs(4), async {
                loop {
                    drain_frames(&mut viewer);
                    if viewer.task.is_finished() { break; }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await;
            assert!(ended.is_ok(), "viewer did not notice the silent host");
            let error = viewer.task.await.unwrap().expect_err("a silent host is an error");
            assert_eq!(error.reason, DisconnectReason::Transient, "a silent host must be reconnectable: {error}");
            let host_result = timeout(Duration::from_secs(4), host).await.expect("host did not notice the silent viewer");
            assert!(host_result.unwrap().is_ok());
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn host_without_capture_tells_viewer_not_to_reconnect() {
            let timings = SessionTimings::default();
            let (_relay, host_ws, viewer_ws) = TestRelay::start(Duration::from_secs(3600)).await;
            let host = tokio::spawn(async move {
                run_host_session_with_data_and_accept_and_2fa_with_factory(
                    host_ws, PASSWORD, SESSION, || None::<FakeCapturer>,
                    |width, height| H264Encoder::new(width, height, 15, 1_000_000),
                    || Box::new(NoopInjector::default()) as Box<dyn InputInjector>,
                    false, false, None, || async { true }, None, timings,
                ).await
            });
            let viewer = spawn_viewer(viewer_ws, timings);
            let error = timeout(Duration::from_secs(10), viewer.task).await.expect("viewer kept waiting").unwrap().expect_err("no capture ends the session");
            assert_eq!(error.reason, DisconnectReason::AuthFailure, "{}", error.message);
            assert_eq!(crate::reconnect::rejection_code(&error.message), Some(reject_code::CAPTURE_UNAVAILABLE));
            assert!(host.await.unwrap().is_err());
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn host_bye_ends_viewer_cleanly_without_reconnect() {
            let timings = SessionTimings::default();
            let (_relay, host_ws, viewer_ws) = TestRelay::start(Duration::from_secs(3600)).await;
            let (ready_tx, ready_rx) = oneshot::channel();
            let host = spawn_host(host_ws, timings, Some(ready_tx));
            let mut viewer = spawn_viewer(viewer_ws, timings);
            let handle = timeout(Duration::from_secs(20), ready_rx).await.unwrap().unwrap();
            first_frame(&mut viewer).await;

            send_bye(&handle.control_sender, &handle.control_sealer).await;
            let ended = timeout(Duration::from_secs(3), async {
                loop {
                    drain_frames(&mut viewer);
                    if viewer.task.is_finished() { break; }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await;
            assert!(ended.is_ok(), "viewer kept running after the host's Bye");
            let error = viewer.task.await.unwrap().expect_err("Bye ends the viewer session");
            assert_eq!(error.message, crate::reconnect::HOST_ENDED_MESSAGE);
            assert_eq!(error.reason, DisconnectReason::CleanUserDisconnect);
            assert!(!crate::reconnect::should_reconnect(error.reason));
            host.abort();
        }
    }
}
