//! Host-role session orchestration.

use crate::{
    capture::{ScapCapturer, ScreenCapturer},
    encode::{self, Encoder, H264Encoder},
    inject::{EnigoInjector, InputInjector, NoopInjector},
    transport::{self, RelayIceSignaling, RelayTransport, ReliableChannel, Transport, TransportReceiver, TransportSender},
};
use protocol::{
    crypto::{Opener, Sealer},
    input::InputEvent,
    kex::derive_keys,
    Tag,
};
use std::{error::Error, fmt::Display, time::Duration};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::WebSocketStream;

const KEX_MSG: u8 = 0x01;
const KEX_CONFIRM: u8 = 0x02;

/// ICE role invariant: the host controls nomination and the viewer accepts it.
/// These values MUST differ for ICE to nominate a connection.
const HOST_ICE_CONTROLLING: bool = true;
const VIEWER_ICE_CONTROLLING: bool = false;

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
}

impl DataHandle {
    pub async fn send_data(&self, sealed: &[u8]) -> Result<(), transport::TransportError> {
        self.sender.send_frame(Tag::Data, sealed).await
    }
}

async fn recv_control(rx: &mut mpsc::Receiver<Vec<u8>>) -> Option<Vec<u8>> { rx.recv().await }

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
    F: FnOnce(u16, u16) -> E,
    I: InputInjector + 'static,
{
    run_host_session_with_data(ws, password, ice_session, capturer, encoder_factory, injector, None).await
}

pub async fn run_host_session_with_data<S, C, E, F, I>(
    ws: WebSocketStream<S>, password: &str, ice_session: &str, mut capturer: C,
    encoder_factory: F, mut injector: I, data_ready: Option<oneshot::Sender<DataHandle>>,
) -> SessionResult<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    C: ScreenCapturer + Send + 'static,
    E: Encoder + Send + 'static,
    F: FnOnce(u16, u16) -> E,
    I: InputInjector + 'static,
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
    let (send_key, recv_key) = derive_keys(&shared, true);
    let mut sealer = Sealer::new(send_key);
    let mut opener = Opener::new(recv_key);
    // The Data channel uses a SEPARATE key (distinct HKDF label) from media, so
    // the two streams' independent nonce counters never collide under a shared
    // key. Sharing the media key here would be catastrophic AEAD nonce reuse.
    let (data_send_key, data_recv_key) = protocol::kex::derive_data_keys(&shared, true);
    let data_sealer = Sealer::new(data_send_key);
    let data_opener = Opener::new(data_recv_key);

    let signaling: RelayIceSignaling<S> = RelayIceSignaling::new(sender.clone(), control_rx, ice_session);
    let relay: RelayTransport<S> = RelayTransport::new(sender, reliable.take_media_rx().ok_or(SessionError("media receiver unavailable"))?);
    let transport = transport::establish(
        HOST_ICE_CONTROLLING,
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
        });
    }
    let (mut transport_tx, mut transport_rx) = transport.into_split();

    let first_frame = loop {
        if let Some(frame) = capturer.next_frame() {
            break frame;
        }
        tokio::time::sleep(Duration::from_millis(1_000 / 15)).await;
    };
    let first_frame = even_frame(first_frame).ok_or(SessionError(
        "screen capture returned an invalid or unusable frame",
    ))?;
    let width = first_frame.width;
    let height = first_frame.height;
    let (input_tx, mut input_rx) = tokio::sync::mpsc::channel(64);
    tokio::spawn(async move {
        loop {
            let received = transport_rx.recv().await;
            let Some((Tag::Input, body)) = received else {
                break;
            };
            let Some(plaintext) = opener.open(&body) else {
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
    let mut seq = 0u64;
    let mut next_frame = tokio::time::Instant::now();
    let mut last_keyframe = next_frame;
    loop {
        tokio::select! {
            Some(event) = input_rx.recv() => injector.inject(&event, u32::from(width), u32::from(height)),
            _ = tokio::time::sleep_until(next_frame) => {
                next_frame += Duration::from_millis(1_000 / u64::from(fps));
                let frame = if seq == 0 { Some(first_frame.clone()) } else { capturer.next_frame().and_then(even_frame) };
                let Some(frame) = frame else { continue };
                if frame.width != width || frame.height != height { continue; }
                if last_keyframe.elapsed() >= Duration::from_secs(2) { encoder.force_intra_frame(); last_keyframe = next_frame; }
                let Some(encoded) = encoder.encode(&frame) else { continue };
                let mut plaintext = encode::video_header(seq, frame.timestamp_us, encoded.keyframe, width, height);
                plaintext.extend_from_slice(&encoded.data);
                let sealed = sealer.seal(&plaintext);
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
) -> Result<(), String>
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

    let signaling: RelayIceSignaling<S> = RelayIceSignaling::new(sender.clone(), control_rx, ice_session);
    let relay: RelayTransport<S> = RelayTransport::new(sender, reliable.take_media_rx().ok_or_else(|| "media receiver unavailable".to_owned())?);
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
        });
    }
    let (transport_tx, transport_rx) = transport.into_split();

    run_viewer_media_loop(transport_tx, transport_rx, frame_sink, input_rx, sealer, opener).await
}

async fn run_viewer_media_loop(
    mut transport_tx: Box<dyn TransportSender>,
    mut transport_rx: Box<dyn TransportReceiver>,
    frame_sink: tokio::sync::mpsc::Sender<Vec<u8>>,
    mut input_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
    mut sealer: Sealer,
    mut opener: Opener,
) -> Result<(), String> {
    loop {
        tokio::select! {
            received = transport_rx.recv() => {
                let Some((Tag::Video, sealed)) = received else { return Ok(()) };
                let Some(plaintext) = opener.open(&sealed) else { continue };
                if frame_sink.send(plaintext).await.is_err() { return Ok(()) }
            }
            input = input_rx.recv() => {
                let Some(bytes) = input else { return Ok(()) };
                let sealed = sealer.seal(&bytes);
                if transport_tx.send(Tag::Input, &sealed).await.is_err() { return Ok(()) }
            }
        }
    }
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
    let capturer = ScapCapturer::new().ok_or(SessionError("Screen capture is unavailable. Grant this app macOS Screen Recording permission and try again."))?;
    let injector: Box<dyn InputInjector> = match EnigoInjector::new() {
        Some(injector) => Box::new(injector),
        None => {
            eprintln!("Input injection unavailable: macOS Accessibility permission is required. Grant it in System Settings, then restart the host.");
            eprintln!("Open Accessibility settings: x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility");
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
    let capturer = ScapCapturer::new().ok_or(SessionError("Screen capture is unavailable. Grant this app macOS Screen Recording permission and try again."))?;
    let injector: Box<dyn InputInjector> = match EnigoInjector::new() {
        Some(injector) => Box::new(injector),
        None => {
            eprintln!("Input injection unavailable: macOS Accessibility permission is required. Grant it in System Settings, then restart the host.");
            eprintln!("Open Accessibility settings: x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility");
            Box::new(NoopInjector::default())
        }
    };
    run_host_session_with_data(
        ws,
        password,
        ice_session,
        capturer,
        |w, h| H264Encoder::new(w, h, 15, 1_000_000),
        injector,
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
        let sealed_video = host_video_sealer.seal(&video);
        host_tx.send(Tag::Video, &sealed_video).await.unwrap();
        assert_eq!(timeout(Duration::from_secs(1), frames.recv()).await.unwrap(), Some(video));

        let input = b"encoded input event".to_vec();
        input_tx.send(input.clone()).await.unwrap();
        let (tag, sealed_input) = timeout(Duration::from_secs(1), host_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(tag, Tag::Input);
        assert_eq!(host_input_opener.open(&sealed_input), Some(input));

        drop(input_tx);
        assert!(timeout(Duration::from_secs(1), viewer).await.unwrap().unwrap().is_ok());
    }
}
