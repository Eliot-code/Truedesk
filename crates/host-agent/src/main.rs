use futures_util::SinkExt;
use protocol::{control::Control, crypto::Sealer, frame, kex::derive_keys, Tag};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

/// Builds the 18-byte video-frame header (big-endian) followed by `payload`:
/// u64 frame_seq, u32 timestamp_us, u8 codec(=1 H264), u8 flags(bit0=keyframe),
/// u16 width, u16 height.
fn video_frame(seq: u64, payload: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(18 + payload.len());
    p.extend_from_slice(&seq.to_be_bytes()); // frame_seq u64
    p.extend_from_slice(&(0u32).to_be_bytes()); // timestamp_us u32
    p.push(1); // codec = H264
    p.push(1); // flags = keyframe
    p.extend_from_slice(&(1280u16).to_be_bytes()); // width
    p.extend_from_slice(&(720u16).to_be_bytes()); // height
    p.extend_from_slice(payload); // encoded bytes
    p
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

    // PoC shared secret; real path derives this via X25519+SPAKE2.
    let (send_key, _recv_key) = derive_keys(&[42u8; 32], true);
    let mut sealer = Sealer::new(send_key);

    let mut seq = 0u64;
    loop {
        let plaintext = video_frame(seq, &[0xABu8; 320]);
        let sealed = sealer.seal(&plaintext);
        ws.send(Message::Binary(frame::wrap(Tag::Video, &sealed)))
            .await
            .unwrap();
        println!("sent frame {seq} ({} bytes sealed)", sealed.len());
        seq += 1;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_frame_header_layout_is_18_bytes_big_endian() {
        let payload = [0xABu8; 320];
        let out = video_frame(7, &payload);

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
