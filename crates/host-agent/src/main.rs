use futures_util::{SinkExt, StreamExt};
use host_agent::{
    capture::{self, ScreenCapturer},
    encode::{self, Encoder},
    inject::{EnigoInjector, InputInjector},
};
use protocol::{
    control::Control,
    crypto::{Opener, Sealer},
    frame,
    input::InputEvent,
    kex::derive_keys,
    Tag,
};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

// KEX sub-tags carried inside Tag::Kex (0x02)
const KEX_MSG: u8 = 0x01;
const KEX_CONFIRM: u8 = 0x02;

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

#[tokio::main]
async fn main() {
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "ws://127.0.0.1:8080/ws".into());
    let peer_id = std::env::args().nth(2).unwrap_or_else(|| "abc".into());

    let (mut ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect");

    ws.send(Message::Binary(frame::wrap(
        Tag::Control,
        &Control::Register { peer_id, ver: 1 }.to_bytes(),
    )))
    .await
    .unwrap();

    // Real SPAKE2 key exchange before any video is sent.
    let password = std::env::args().nth(3).unwrap_or_else(|| "test".into());

    // 1. start SPAKE2 (host = side A) and send our message.
    let (state, my_msg) = protocol::pake::start(password.as_bytes(), true);
    ws.send(Message::Binary(frame::wrap(
        Tag::Kex,
        &kex_body(KEX_MSG, &my_msg),
    )))
    .await
    .unwrap();

    // 2. receive the viewer's SPAKE2 message (skip non-Kex frames, e.g. `paired`).
    let shared = loop {
        match ws.next().await {
            Some(Ok(Message::Binary(m))) => {
                if let Some((Tag::Kex, body)) = frame::unwrap(&m) {
                    if let Some((KEX_MSG, msg)) = parse_kex(body) {
                        break protocol::pake::finish(state, msg).expect("valid spake2 msg");
                    }
                }
            }
            Some(Ok(_)) => {} // tolerate/skip non-binary control frames
            _ => panic!("connection closed during KEX"),
        }
    };

    // 3. exchange confirmation MACs.
    let my_confirm = protocol::pake::confirm_tag(&shared, true);
    ws.send(Message::Binary(frame::wrap(
        Tag::Kex,
        &kex_body(KEX_CONFIRM, &my_confirm),
    )))
    .await
    .unwrap();

    let mut verified = false;
    while !verified {
        match ws.next().await {
            Some(Ok(Message::Binary(m))) => {
                if let Some((Tag::Kex, body)) = frame::unwrap(&m) {
                    if let Some((KEX_CONFIRM, tag)) = parse_kex(body) {
                        // peer is the viewer (peer_is_host = false)
                        if !protocol::pake::verify_peer(&shared, false, tag) {
                            panic!("key confirmation FAILED — wrong password or tampering");
                        }
                        verified = true;
                    }
                }
            }
            Some(Ok(_)) => {} // tolerate/skip non-binary control frames
            _ => panic!("connection closed during KEX confirm"),
        }
    }

    // 4. derive real per-direction keys (replaces the fixed [42;32]).
    let (send_key, recv_key) = derive_keys(&shared, true);
    let mut sealer = Sealer::new(send_key);
    let mut opener = Opener::new(recv_key);

    let mut capturer = match capture::ScapCapturer::new() {
        Some(capturer) => capturer,
        None => {
            eprintln!("Screen capture is unavailable. Grant this app macOS Screen Recording permission and try again.");
            return;
        }
    };
    let first_frame = loop {
        if let Some(frame) = capturer.next_frame() {
            break frame;
        }
        tokio::time::sleep(Duration::from_millis(1_000 / 15)).await;
    };
    let first_frame = match even_frame(first_frame) {
        Some(frame) => frame,
        None => {
            eprintln!("Screen capture returned an invalid or unusable frame.");
            return;
        }
    };
    let width = first_frame.width;
    let height = first_frame.height;

    let mut injector: Box<dyn InputInjector> = match EnigoInjector::new() {
        Some(injector) => Box::new(injector),
        None => {
            eprintln!("Input injection unavailable: macOS Accessibility permission is required. Grant it in System Settings, then restart the host.");
            eprintln!("Open Accessibility settings: x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility");
            Box::new(host_agent::inject::NoopInjector::default())
        }
    };

    // Split the WebSocket so incoming input is read independently of the video
    // producer. The first captured frame supplies the single-display dimensions.
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

    let fps = 15;
    let mut encoder = encode::H264Encoder::new(width, height, fps, 1_000_000);
    let mut seq = 0u64;
    let mut next_frame = tokio::time::Instant::now();
    let mut last_keyframe = next_frame;

    loop {
        tokio::select! {
            Some(event) = input_rx.recv() => {
                injector.inject(&event, u32::from(width), u32::from(height));
            }
            _ = tokio::time::sleep_until(next_frame) => {
                next_frame += Duration::from_millis(1_000 / u64::from(fps));
                let frame = if seq == 0 {
                    Some(first_frame.clone())
                } else {
                    capturer.next_frame().and_then(even_frame)
                };
                let Some(frame) = frame else { continue };
                if frame.width != width || frame.height != height { continue };
                if last_keyframe.elapsed() >= Duration::from_secs(2) {
                    encoder.force_intra_frame();
                    last_keyframe = next_frame;
                }
                let Some(encoded) = encoder.encode(&frame) else { continue };
                let mut plaintext =
                    encode::video_header(seq, frame.timestamp_us, encoded.keyframe, width, height);
                plaintext.extend_from_slice(&encoded.data);
                let sealed = sealer.seal(&plaintext);
                ws_sink
                    .send(Message::Binary(frame::wrap(Tag::Video, &sealed)))
                    .await
                    .unwrap();
                println!("sent frame {seq} ({} bytes sealed)", sealed.len());
                seq += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_frame_header_layout_is_18_bytes_big_endian() {
        let payload = [0xABu8; 320];
        let mut out = encode::video_header(7, 0, true, 1280, 720);
        out.extend_from_slice(&payload);

        assert_eq!(out.len(), 18 + payload.len());
        assert_eq!(&out[0..8], &7u64.to_be_bytes()); // frame_seq
        assert_eq!(&out[8..12], &0u32.to_be_bytes()); // timestamp_us
        assert_eq!(out[12], 1); // codec = H264
        assert_eq!(out[13], 1); // flags bit0 = keyframe
        assert_eq!(&out[14..16], &1280u16.to_be_bytes()); // width
        assert_eq!(&out[16..18], &720u16.to_be_bytes()); // height
        assert_eq!(&out[18..], &payload[..]); // payload follows header
    }
}
