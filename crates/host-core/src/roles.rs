//! Host-role session orchestration.

use crate::{
    audio,
    capture::{ScapCapturer, ScreenCapturer},
    encode::{self, Encoder, H264Encoder},
    inject::{EnigoInjector, InputInjector, NoopInjector},
    transport::{self, RelayIceSignaling, RelayTransport, ReliableChannel, Transport, TransportReceiver, TransportSender},
};
use crate::reconnect::{classify_viewer_error, DisconnectReason};
use protocol::{
    control::Control,
    crypto::{Opener, Sealer},
    input::InputEvent,
    kex::derive_keys,
    Tag,
};
use std::{error::Error, fmt::Display, future::Future, time::Duration};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::WebSocketStream;

const KEX_MSG: u8 = 0x01;
const KEX_CONFIRM: u8 = 0x02;

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
        .any(|time| totp_lite::totp_custom::<totp_lite::Sha1>(30, 6, &decoded, time) == code)
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
    pub control_sealer: Sealer,
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

fn split_session_control(mut rx: mpsc::Receiver<Vec<u8>>, session: String) -> (mpsc::Receiver<Vec<u8>>, mpsc::Receiver<Vec<u8>>) {
    let (ice_tx, ice_rx) = mpsc::channel(64);
    let (session_tx, session_rx) = mpsc::channel(64);
    tokio::spawn(async move {
        while let Some(body) = rx.recv().await {
            let is_ice = matches!(Control::from_bytes(&body), Some(Control::Ice { session: body_session, .. }) if body_session == session);
            let result = if is_ice { ice_tx.send(body).await } else { session_tx.send(body).await };
            if result.is_err() { break; }
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
    F: Fn(u16, u16) -> E,
    I: InputInjector + 'static,
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
    F: Fn(u16, u16) -> E,
    I: InputInjector + 'static,
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
    F: Fn(u16, u16) -> E,
    I: InputInjector + 'static,
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
    ws: WebSocketStream<S>, password: &str, ice_session: &str, mut capturer: C,
    encoder_factory: F, mut injector: I, unattended_enabled: bool,
    twofa_enabled: bool, totp_secret: Option<&str>, approve: A,
    data_ready: Option<oneshot::Sender<DataHandle>>,
) -> SessionResult<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    C: ScreenCapturer + Send + 'static,
    E: Encoder + Send + 'static,
    F: Fn(u16, u16) -> E,
    I: InputInjector + 'static,
    A: FnOnce() -> Fut,
    Fut: Future<Output = bool>,
{
    let mut reliable = ReliableChannel::spawn(ws);
    let sender = reliable.sender();
    let mut control_rx = reliable.take_control_rx().ok_or(SessionError("control receiver unavailable"))?;
    let data_rx = reliable.take_data_rx().ok_or(SessionError("data receiver unavailable"))?;
    let (state, my_msg) = protocol::pake::start(password.as_bytes(), true);
    sender.send_frame(Tag::Kex, &kex_body(KEX_MSG, &my_msg)).await?;
    let shared = loop {
        match recv_control(&mut control_rx).await {
            Some(body) => if let Some((KEX_MSG, msg)) = parse_kex(&body) {
                break protocol::pake::finish(state, msg).ok_or(SessionError("invalid SPAKE2 message"))?;
            },
            None => return Err(Box::new(SessionError("connection closed during KEX"))),
        }
    };
    let my_confirm = protocol::pake::confirm_tag(&shared, true);
    sender.send_frame(Tag::Kex, &kex_body(KEX_CONFIRM, &my_confirm)).await?;
    loop {
        match recv_control(&mut control_rx).await {
            Some(body) => if let Some((KEX_CONFIRM, tag)) = parse_kex(&body) {
                if !protocol::pake::verify_peer(&shared, false, tag) { return Err(Box::new(SessionError("key confirmation failed"))); }
                break;
            },
            None => return Err(Box::new(SessionError("connection closed during KEX confirmation"))),
        }
    }
    let (control_send_key, control_recv_key) = protocol::kex::derive_control_keys(&shared, true);
    let mut control_sealer = Sealer::new(control_send_key);
    let mut control_opener = Opener::new(control_recv_key);
    let code_valid = if unattended_enabled && twofa_enabled {
        let body = tokio::time::timeout(Duration::from_secs(30), recv_control(&mut control_rx)).await.ok().flatten();
        let code = body.and_then(|body| control_opener.open(&[Tag::Control as u8], &body)).and_then(|body| match Control::from_bytes(&body) {
            Some(Control::AuthCode { code }) => Some(code),
            _ => None,
        });
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |duration| duration.as_secs());
        code.as_deref().zip(totp_secret).is_some_and(|(code, secret)| totp_is_valid(secret, code, now))
    } else { true };
    if (unattended_enabled && twofa_enabled && !code_valid)
        || (!should_auto_accept_with_2fa(unattended_enabled, true, twofa_enabled, code_valid) && !approve().await) {
        return Err(Box::new(SessionError("session rejected")));
    }
    let (send_key, recv_key) = derive_keys(&shared, true);
    let mut sealer = Sealer::new(send_key);
    let mut opener = Opener::new(recv_key);
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
        transport::default_agent_config(),
        Duration::from_secs(3),
    )
    .await;
    // Send the monitor list first, then hand the SAME control sealer to the
    // data channel. Host->viewer control frames must share one sealer (one
    // nonce sequence) on this key: a second Sealer::new on control_send_key
    // would restart the counter at 0 and reuse a nonce with MonitorList.
    let monitor_list = Control::MonitorList { monitors: capturer.monitors() };
    reliable.sender().send_frame(Tag::Control, &control_sealer.seal(&[Tag::Control as u8], &monitor_list.to_bytes())).await?;
    if let Some(ready) = data_ready {
        let _ = ready.send(DataHandle {
            sender: reliable.sender(),
            receiver: data_rx,
            sealer: data_sealer,
            opener: data_opener,
            control_sender: reliable.sender(),
            control_sealer,
        });
    }
    let (mut transport_tx, mut transport_rx) = transport.into_split();
    let mut audio_rx = audio::spawn_capture();

    let first_frame = loop {
        if let Some(frame) = capturer.next_frame() {
            break frame;
        }
        tokio::time::sleep(Duration::from_millis(1_000 / 15)).await;
    };
    let first_frame = even_frame(first_frame).ok_or(SessionError(
        "screen capture returned an invalid or unusable frame",
    ))?;
    let mut width = first_frame.width;
    let mut height = first_frame.height;
    let (input_tx, mut input_rx) = tokio::sync::mpsc::channel(64);
    tokio::spawn(async move {
        loop {
            let received = transport_rx.recv().await;
            let Some((Tag::Input, body)) = received else {
                break;
            };
            let Some(plaintext) = opener.open(&[Tag::Input as u8], &body) else {
                continue;
            };
            let Some((event, _timestamp_us)) = InputEvent::from_bytes(&plaintext) else {
                continue;
            };
            if input_tx.send(event).await.is_err() {
                break;
            }
        }
    });
    let fps: u32 = 15;
    let mut encoder = encoder_factory(width, height);
    let mut pending_first_frame = Some(first_frame);
    let mut seq = 0u64;
    let mut next_frame = tokio::time::Instant::now();
    let mut last_keyframe = next_frame;
    loop {
        tokio::select! {
            Some(audio_frame) = audio_rx.recv() => {
                let sealed = sealer.seal(&[Tag::Audio as u8], &audio_frame.to_bytes());
                let _ = transport_tx.send(Tag::Audio, &sealed).await;
            },
            Some(body) = session_control_rx.recv() => {
                if let Some(plaintext) = control_opener.open(&[Tag::Control as u8], &body) {
                    if let Some(Control::SelectMonitor { id }) = Control::from_bytes(&plaintext) {
                        if capturer.select_monitor(id) {
                            if let Some(frame) = capturer.next_frame().and_then(even_frame) {
                                width = frame.width;
                                height = frame.height;
                                encoder = encoder_factory(width, height);
                                pending_first_frame = Some(frame);
                                seq = 0;
                            }
                        }
                    }
                }
            },
            Some(event) = input_rx.recv() => injector.inject(&event, u32::from(width), u32::from(height)),
            _ = tokio::time::sleep_until(next_frame) => {
                next_frame += Duration::from_millis(1_000 / u64::from(fps));
                let frame = pending_first_frame.take().or_else(|| capturer.next_frame().and_then(even_frame));
                let Some(frame) = frame else { continue };
                if frame.width != width || frame.height != height { continue; }
                if last_keyframe.elapsed() >= Duration::from_secs(2) { encoder.force_intra_frame(); last_keyframe = next_frame; }
                let Some(encoded) = encoder.encode(&frame) else { continue };
                let mut plaintext = encode::video_header(seq, frame.timestamp_us, encoded.keyframe, width, height);
                plaintext.extend_from_slice(&encoded.data);
                let sealed = sealer.seal(&[Tag::Video as u8], &plaintext);
                transport_tx.send(Tag::Video, &sealed).await?;
                seq += 1;
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
    let result = run_viewer_session_inner(ws, password, ice_session, frame_sink, input_rx, data_ready, monitor_sink, audio_sink, auth_code).await;
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

    let shared = loop {
        match recv_control(&mut control_rx).await {
            Some(body) => if let Some((KEX_MSG, msg)) = parse_kex(&body) {
                break protocol::pake::finish(state, msg).ok_or_else(|| "invalid SPAKE2 message".to_owned())?;
            },
            None => return Err("connection closed during KEX".to_owned()),
        }
    };

    let my_confirm = protocol::pake::confirm_tag(&shared, false);
    sender.send_frame(Tag::Kex, &kex_body(KEX_CONFIRM, &my_confirm)).await.map_err(|error| error.to_string())?;
    loop {
        match recv_control(&mut control_rx).await {
            Some(body) => if let Some((KEX_CONFIRM, tag)) = parse_kex(&body) {
                if !protocol::pake::verify_peer(&shared, true, tag) { return Err("key confirmation failed".to_owned()); }
                break;
            },
            None => return Err("connection closed during KEX confirmation".to_owned()),
        }
    }

    let (send_key, recv_key) = derive_keys(&shared, false);
    let sealer = Sealer::new(send_key);
    let opener = Opener::new(recv_key);
    // Separate Data-channel key (see host session) — avoids nonce reuse.
    let (data_send_key, data_recv_key) = protocol::kex::derive_data_keys(&shared, false);
    let data_sealer = Sealer::new(data_send_key);
    let data_opener = Opener::new(data_recv_key);
    let (control_send_key, control_recv_key) = protocol::kex::derive_control_keys(&shared, false);
    let mut control_sealer = Sealer::new(control_send_key);
    let control_opener = Opener::new(control_recv_key);
    if let Some(code) = auth_code {
        let body = Control::AuthCode { code }.to_bytes();
        sender.send_frame(Tag::Control, &control_sealer.seal(&[Tag::Control as u8], &body)).await.map_err(|error| error.to_string())?;
    }
    let (ice_rx, session_control_rx) = split_session_control(control_rx, ice_session.to_owned());

    let signaling: RelayIceSignaling<S> = RelayIceSignaling::new(sender.clone(), ice_rx, ice_session);
    let relay: RelayTransport<S> = RelayTransport::new(sender.clone(), reliable.take_media_rx().ok_or_else(|| "media receiver unavailable".to_owned())?);
    let transport = transport::establish(
        VIEWER_ICE_CONTROLLING,
        signaling,
        relay,
        transport::default_agent_config(),
        Duration::from_secs(3),
    )
    .await;
    if let Some(ready) = data_ready {
        let _ = ready.send(DataHandle {
            sender: reliable.sender(),
            receiver: data_rx,
            sealer: data_sealer,
            opener: data_opener,
            control_sender: reliable.sender(),
            control_sealer,
        });
    }
    let (transport_tx, transport_rx) = transport.into_split();

    run_viewer_media_loop_with_control(transport_tx, transport_rx, frame_sink, input_rx, sealer, opener, session_control_rx, control_opener, monitor_sink, audio_sink).await
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
    run_viewer_media_loop_with_control(transport_tx, transport_rx, frame_sink, input_rx, sealer, opener, rx, Opener::new([0; 32]), None, None).await
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
    mut control_opener: Opener,
    monitor_sink: Option<tokio::sync::mpsc::Sender<Vec<u8>>>,
    audio_sink: Option<tokio::sync::mpsc::Sender<Vec<u8>>>,
) -> Result<(), String> {
    loop {
        tokio::select! {
            Some(body) = control_rx.recv() => {
                if let Some(plaintext) = control_opener.open(&[Tag::Control as u8], &body) {
                    if let Some(Control::MonitorList { .. }) = Control::from_bytes(&plaintext) {
                        if let Some(sink) = &monitor_sink { let _ = sink.send(plaintext).await; }
                    }
                }
            },
            received = transport_rx.recv() => {
                let Some((tag, sealed)) = received else { return Err("connection closed during session".into()) };
                let Some(plaintext) = opener.open(&[tag as u8], &sealed) else { continue };
                match tag {
                    Tag::Video => if frame_sink.send(plaintext).await.is_err() { return Err("viewer frame channel closed".into()) },
                    Tag::Audio => if let Some(sink) = &audio_sink { if sink.send(plaintext).await.is_err() { return Err("viewer audio channel closed".into()) } },
                    _ => {}
                }
            }
            input = input_rx.recv() => {
                let Some(bytes) = input else { return Ok(()) };
                let sealed = sealer.seal(&[Tag::Input as u8], &bytes);
                if transport_tx.send(Tag::Input, &sealed).await.is_err() { return Err("viewer input transport closed".into()) }
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
    eprintln!("Input injection unavailable: macOS Accessibility permission is required. Grant it in System Settings, then restart the host.");
    eprintln!("Open Accessibility settings: x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility");
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn report_input_unavailable() {
    // Enigo is backed by SendInput on Windows and XTest/uinput on Linux.
    eprintln!("Input injection unavailable: the platform input backend could not be initialized.");
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
    let capturer = ScapCapturer::new().ok_or(SessionError(capture_unavailable_message()))?;
    let injector: Box<dyn InputInjector> = match EnigoInjector::new() {
        Some(injector) => Box::new(injector),
        None => {
            report_input_unavailable();
            Box::new(NoopInjector::default())
        }
    };
    run_host_session(
        ws,
        password,
        ice_session,
        capturer,
        |w, h| H264Encoder::new(w, h, 15, 1_000_000),
        injector,
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
    let capturer = ScapCapturer::new().ok_or(SessionError(capture_unavailable_message()))?;
    let injector: Box<dyn InputInjector> = match EnigoInjector::new() {
        Some(injector) => Box::new(injector),
        None => {
            report_input_unavailable();
            Box::new(NoopInjector::default())
        }
    };
    run_host_session_with_data_and_accept_and_2fa(
        ws,
        password,
        ice_session,
        capturer,
        |w, h| H264Encoder::new(w, h, 15, 1_000_000),
        injector,
        unattended_enabled,
        twofa_enabled,
        totp_secret,
        approve,
        Some(data_ready),
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

        let video = b"18-byte-header-plus-h264".to_vec();
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
}
