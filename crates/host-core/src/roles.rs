//! Host-role session orchestration.

use crate::{
    capture::{ScapCapturer, ScreenCapturer},
    encode::{self, Encoder, H264Encoder},
    inject::{EnigoInjector, InputInjector, NoopInjector},
};
use futures_util::{SinkExt, StreamExt};
use protocol::{
    crypto::{Opener, Sealer},
    frame,
    input::InputEvent,
    kex::derive_keys,
    Tag,
};
use std::{error::Error, fmt::Display, time::Duration};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};

const KEX_MSG: u8 = 0x01;
const KEX_CONFIRM: u8 = 0x02;

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
    mut ws: WebSocketStream<S>,
    password: &str,
    mut capturer: C,
    encoder_factory: F,
    mut injector: I,
) -> SessionResult<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    C: ScreenCapturer + Send + 'static,
    E: Encoder + Send + 'static,
    F: FnOnce(u16, u16) -> E,
    I: InputInjector + 'static,
{
    let (state, my_msg) = protocol::pake::start(password.as_bytes(), true);
    ws.send(Message::Binary(frame::wrap(
        Tag::Kex,
        &kex_body(KEX_MSG, &my_msg),
    )))
    .await?;
    let shared = loop {
        match ws.next().await {
            Some(Ok(Message::Binary(m))) => {
                if let Some((Tag::Kex, body)) = frame::unwrap(&m) {
                    if let Some((KEX_MSG, msg)) = parse_kex(body) {
                        break protocol::pake::finish(state, msg)
                            .ok_or(SessionError("invalid SPAKE2 message"))?;
                    }
                }
            }
            Some(Ok(_)) => {}
            _ => return Err(Box::new(SessionError("connection closed during KEX"))),
        }
    };
    let my_confirm = protocol::pake::confirm_tag(&shared, true);
    ws.send(Message::Binary(frame::wrap(
        Tag::Kex,
        &kex_body(KEX_CONFIRM, &my_confirm),
    )))
    .await?;
    loop {
        match ws.next().await {
            Some(Ok(Message::Binary(m))) => {
                if let Some((Tag::Kex, body)) = frame::unwrap(&m) {
                    if let Some((KEX_CONFIRM, tag)) = parse_kex(body) {
                        if !protocol::pake::verify_peer(&shared, false, tag) {
                            return Err(Box::new(SessionError("key confirmation failed")));
                        }
                        break;
                    }
                }
            }
            Some(Ok(_)) => {}
            _ => {
                return Err(Box::new(SessionError(
                    "connection closed during KEX confirmation",
                )))
            }
        }
    }
    let (send_key, recv_key) = derive_keys(&shared, true);
    let mut sealer = Sealer::new(send_key);
    let mut opener = Opener::new(recv_key);

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
    let (mut ws_sink, mut ws_stream) = ws.split();
    let (input_tx, mut input_rx) = tokio::sync::mpsc::channel(64);
    tokio::spawn(async move {
        while let Some(message) = ws_stream.next().await {
            let Ok(Message::Binary(message)) = message else {
                break;
            };
            let Some((Tag::Input, body)) = frame::unwrap(&message) else {
                continue;
            };
            let Some(plaintext) = opener.open(body) else {
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
                ws_sink.send(Message::Binary(frame::wrap(Tag::Video, &sealed))).await?;
                println!("sent frame {seq} ({} bytes sealed)", sealed.len());
                seq += 1;
            }
        }
    }
}

/// Run a host session with the production capture, H.264 encoder, and Enigo injector.
pub async fn run_default_host_session<S>(
    ws: WebSocketStream<S>,
    password: &str,
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
        capturer,
        |w, h| H264Encoder::new(w, h, 15, 1_000_000),
        injector,
    )
    .await
}
