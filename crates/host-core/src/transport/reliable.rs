//! Single-owner, tagged relay WebSocket channel.
//!
//! The WebSocket is split exactly once: one task owns the read half and
//! demultiplexes frames, and one task owns the write half.  Consumers receive
//! independent channels, so receiving media or setup traffic never requires a
//! lock held by the writer.

use super::TransportError;
use futures_util::{SinkExt, StreamExt};
use protocol::{frame, Tag};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use std::collections::VecDeque;
use std::sync::{atomic::{AtomicBool, Ordering}, Arc};
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};

const CHANNEL_CAPACITY: usize = 128;
/// Upper bound on Data frames buffered while the consumer is slow/absent, so a
/// peer flooding Tag::Data cannot grow memory without limit. On overflow the
/// oldest pending Data frame is dropped.
const MAX_PENDING_DATA: usize = 256;
/// File transfers use 64 KiB chunks plus a small header and AEAD overhead.
/// This leaves room for larger future data messages and clipboard payloads.
const MAX_PENDING_DATA_BYTES: usize = 4 * 1024 * 1024 - MAX_DATA_BODY;
const MAX_PENDING_MEDIA_BYTES: usize = 8 * 1024 * 1024 - MAX_VIDEO_BODY;
const MAX_PENDING_MEDIA: usize = 128;
/// KEX messages are small SPAKE2 values; 4 KiB rejects malformed growth.
const MAX_KEX_BODY: usize = 4 * 1024;
/// Control is JSON ICE/monitor/auth traffic; 64 KiB is far above normal use.
const MAX_CONTROL_BODY: usize = 64 * 1024;
/// Encoded video frames are normally much smaller; 4 MiB bounds keyframes.
const MAX_VIDEO_BODY: usize = 4 * 1024 * 1024;
/// Opus is a 20 ms packet; 64 KiB is a generous malformed-packet ceiling.
const MAX_AUDIO_BODY: usize = 64 * 1024;
/// InputEvent encodings are at most 17 bytes; allow protocol headroom.
const MAX_INPUT_BODY: usize = 1024;
/// Data frames carry a 64 KiB file chunk plus framing/sealing overhead.
const MAX_DATA_BODY: usize = 256 * 1024;

/// A reliable tagged channel over a relay WebSocket.
pub struct ReliableChannel<S> {
    sender: ReliableSender,
    data_rx: Option<mpsc::Receiver<Vec<u8>>>,
    media_rx: Option<mpsc::Receiver<(Tag, Vec<u8>)>>,
    control_rx: Option<mpsc::Receiver<Vec<u8>>>,
    reader_task: JoinHandle<()>,
    writer_task: JoinHandle<()>,
    application_traffic_enabled: Arc<AtomicBool>,
    _stream: std::marker::PhantomData<S>,
}

/// Cloneable sending handle for the session's single WebSocket writer.
#[derive(Clone)]
pub struct ReliableSender {
    writer_tx: mpsc::Sender<Vec<u8>>,
}

impl ReliableSender {
    pub async fn send_frame(&self, tag: Tag, body: &[u8]) -> Result<(), TransportError> {
        self.writer_tx
            .send(frame::wrap(tag, body))
            .await
            .map_err(|_| TransportError::Closed)
    }
}

impl<S> ReliableChannel<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    /// Start the reader and writer tasks for an already connected WebSocket.
    pub fn spawn(ws: WebSocketStream<S>) -> Self {
        let (sink, mut stream) = ws.split();
        let (writer_tx, mut writer_rx) = mpsc::channel::<Vec<u8>>(CHANNEL_CAPACITY);
        // One slot makes the staging-byte budgets below cover the channel too
        // (the slot is bounded by the largest frame for its path).
        let (data_tx, data_rx) = mpsc::channel::<Vec<u8>>(1);
        let (media_tx, media_rx) = mpsc::channel::<(Tag, Vec<u8>)>(1);
        let (control_tx, control_rx) = mpsc::channel(CHANNEL_CAPACITY);
        let application_traffic_enabled = Arc::new(AtomicBool::new(false));
        let reader_application_traffic_enabled = Arc::clone(&application_traffic_enabled);

        let writer_task = tokio::spawn(async move {
            let mut sink = sink;
            while let Some(bytes) = writer_rx.recv().await {
                if sink.send(Message::Binary(bytes.into())).await.is_err() {
                    break;
                }
            }
        });

        let reader_task = tokio::spawn(async move {
            let mut pending_data = VecDeque::new();
            let mut pending_data_bytes = 0usize;
            let mut pending_media = VecDeque::new();
            let mut pending_media_bytes = 0usize;
            loop {
                let message = if let Some(body) = pending_data.front().cloned() {
                    tokio::select! {
                        result = data_tx.send(body) => {
                            if result.is_err() { break; }
                            let sent = pending_data.pop_front().expect("pending data exists");
                            pending_data_bytes -= sent.len();
                            continue;
                        }
                        message = stream.next() => message,
                    }
                } else if let Some(item) = pending_media.front().cloned() {
                    tokio::select! {
                        result = media_tx.send(item) => {
                            if result.is_err() { break; }
                            let (_, sent) = pending_media.pop_front().expect("pending media exists");
                            pending_media_bytes -= sent.len();
                            continue;
                        }
                        message = stream.next() => message,
                    }
                } else {
                    stream.next().await
                };
                let Some(message) = message else { break };
                let bytes = match message {
                    Ok(Message::Binary(bytes)) => bytes,
                    // WebSocket-level pings (answered by tungstenite) and
                    // pongs are liveness traffic, and Text is not part of the
                    // protocol: none of them may tear the session down.
                    Ok(Message::Ping(_) | Message::Pong(_) | Message::Text(_) | Message::Frame(_)) => continue,
                    Ok(Message::Close(_)) | Err(_) => break,
                };
                let Some((tag, body)) = frame::unwrap(&bytes) else {
                    continue;
                };
                if body.len() > max_body_size(tag) { continue; }

                match tag {
                    // Data is reliable and ordered. A full data queue applies
                    // backpressure to Data delivery via the select! above, while
                    // the demux keeps reading media so a slow Data consumer
                    // cannot starve it. The pending buffer is bounded so a peer
                    // flooding Data at a wedged/absent consumer cannot grow
                    // memory without limit: on overflow the connection is closed
                    // so reliable file-transfer semantics are preserved.
                    Tag::Data => {
                        if !reader_application_traffic_enabled.load(Ordering::Acquire) { continue; }
                        if enqueue_data(&mut pending_data, &mut pending_data_bytes, body).is_err() { break; }
                    }
                    // Media is lossy under pressure; drop the oldest item so
                    // the consumer sees the freshest frame available.
                    Tag::Video | Tag::Audio | Tag::Input => {
                        if !reader_application_traffic_enabled.load(Ordering::Acquire) { continue; }
                        enqueue_media(&mut pending_media, &mut pending_media_bytes, tag, body);
                    }
                    // Control and Kex are setup/control payloads. The tag is
                    // intentionally removed here; both use the same body API.
                    Tag::Control | Tag::Kex => {
                        if control_tx.send(body.to_vec()).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });

        Self {
            sender: ReliableSender { writer_tx },
            data_rx: Some(data_rx),
            media_rx: Some(media_rx),
            control_rx: Some(control_rx),
            reader_task,
            writer_task,
            application_traffic_enabled,
            _stream: std::marker::PhantomData,
        }
    }

    /// Queue a tagged frame for the single WebSocket writer.
    pub async fn send_frame(&self, tag: Tag, body: &[u8]) -> Result<(), TransportError> {
        self.sender.send_frame(tag, body).await
    }

    pub fn sender(&self) -> ReliableSender { self.sender.clone() }

    pub fn enable_application_traffic(&self) {
        self.application_traffic_enabled.store(true, Ordering::Release);
    }

    /// Take the ordered, reliable Data body stream.
    pub fn take_data_rx(&mut self) -> Option<mpsc::Receiver<Vec<u8>>> {
        self.data_rx.take()
    }

    /// Take the lossy Video/Audio/Input stream, retaining each frame's tag.
    pub fn take_media_rx(&mut self) -> Option<mpsc::Receiver<(Tag, Vec<u8>)>> {
        self.media_rx.take()
    }

    /// Take the setup/control body stream for Control and Kex frames.
    pub fn take_control_rx(&mut self) -> Option<mpsc::Receiver<Vec<u8>>> {
        self.control_rx.take()
    }
}

fn max_body_size(tag: Tag) -> usize {
    match tag {
        Tag::Kex => MAX_KEX_BODY,
        Tag::Control => MAX_CONTROL_BODY,
        Tag::Video => MAX_VIDEO_BODY,
        Tag::Audio => MAX_AUDIO_BODY,
        Tag::Input => MAX_INPUT_BODY,
        Tag::Data => MAX_DATA_BODY,
    }
}

fn enqueue_data(queue: &mut VecDeque<Vec<u8>>, bytes: &mut usize, body: &[u8]) -> Result<(), ()> {
    if queue.len() >= MAX_PENDING_DATA || body.len() > MAX_PENDING_DATA_BYTES.saturating_sub(*bytes) { return Err(()); }
    let body = body.to_vec();
    *bytes += body.len();
    queue.push_back(body);
    Ok(())
}

fn enqueue_media(queue: &mut VecDeque<(Tag, Vec<u8>)>, bytes: &mut usize, tag: Tag, body: &[u8]) {
    if body.len() > MAX_PENDING_MEDIA_BYTES { return; }
    while queue.len() >= MAX_PENDING_MEDIA || (*bytes).saturating_add(body.len()) > MAX_PENDING_MEDIA_BYTES {
        if let Some((_, old_body)) = queue.pop_front() { *bytes -= old_body.len(); } else { break; }
    }
    let body = body.to_vec();
    *bytes += body.len();
    queue.push_back((tag, body));
}

impl<S> Drop for ReliableChannel<S> {
    fn drop(&mut self) {
        self.reader_task.abort();
        self.writer_task.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{timeout, Duration};
    use tokio_tungstenite::{tungstenite::protocol::Role, WebSocketStream};

    #[tokio::test]
    async fn interleaved_data_and_media_are_demultiplexed_without_send_stall() {
        let (left, right) = tokio::io::duplex(64 * 1024);
        let left = WebSocketStream::from_raw_socket(left, Role::Client, None).await;
        let right = WebSocketStream::from_raw_socket(right, Role::Server, None).await;
        let a = ReliableChannel::spawn(left);
        let mut b = ReliableChannel::spawn(right);
        b.enable_application_traffic();
        let mut data_rx = b.take_data_rx().unwrap();
        let mut media_rx = b.take_media_rx().unwrap();

        let data_reader = tokio::spawn(async move {
            let mut received = Vec::new();
            for _ in 0..32 {
                received.push(data_rx.recv().await.unwrap());
            }
            received
        });
        let media_reader = tokio::spawn(async move {
            let mut received = Vec::new();
            for _ in 0..64 {
                received.push(media_rx.recv().await.unwrap());
            }
            received
        });

        let sends = async {
            for index in 0..32u8 {
                a.send_frame(Tag::Data, &[index]).await.unwrap();
                a.send_frame(Tag::Video, &[index + 32]).await.unwrap();
                a.send_frame(Tag::Audio, &[index + 64]).await.unwrap();
            }
        };
        timeout(Duration::from_secs(2), sends)
            .await
            .expect("interleaved sends must complete");

        let data = timeout(Duration::from_secs(2), data_reader).await.expect("data delivery").unwrap();
        let media = timeout(Duration::from_secs(2), media_reader).await.expect("media delivery").unwrap();
        assert_eq!(data, (0..32u8).map(|n| vec![n]).collect::<Vec<_>>());
        assert_eq!(
            media,
            (0..32u8).flat_map(|n| [(Tag::Video, vec![n + 32]), (Tag::Audio, vec![n + 64])])
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn media_can_flow_while_data_consumer_is_wedged_until_data_queue_fills() {
        let (left, right) = tokio::io::duplex(64 * 1024);
        let left = WebSocketStream::from_raw_socket(left, Role::Client, None).await;
        let right = WebSocketStream::from_raw_socket(right, Role::Server, None).await;
        let a = ReliableChannel::spawn(left);
        let mut b = ReliableChannel::spawn(right);
        b.enable_application_traffic();
        let _data_rx = b.take_data_rx().unwrap();
        let mut media_rx = b.take_media_rx().unwrap();

        a.send_frame(Tag::Data, b"wedged consumer").await.unwrap();
        a.send_frame(Tag::Video, b"still flowing").await.unwrap();
        assert_eq!(
            timeout(Duration::from_secs(1), media_rx.recv())
                .await
                .unwrap(),
            Some((Tag::Video, b"still flowing".to_vec()))
        );
    }

    #[tokio::test]
    async fn ping_pong_and_text_frames_do_not_close_the_channel() {
        let (left, right) = tokio::io::duplex(64 * 1024);
        let mut raw = WebSocketStream::from_raw_socket(left, Role::Client, None).await;
        let right = WebSocketStream::from_raw_socket(right, Role::Server, None).await;
        let mut channel = ReliableChannel::spawn(right);
        let mut control_rx = channel.take_control_rx().unwrap();

        raw.send(Message::Ping(b"are you there".to_vec().into())).await.unwrap();
        raw.send(Message::Pong(Vec::new().into())).await.unwrap();
        raw.send(Message::Text("not protocol".into())).await.unwrap();
        raw.send(Message::Binary(frame::wrap(Tag::Control, b"still open").into())).await.unwrap();
        assert_eq!(
            timeout(Duration::from_secs(1), control_rx.recv()).await.unwrap(),
            Some(b"still open".to_vec())
        );
    }

    #[test]
    fn byte_budgets_bound_data_and_drop_oldest_media() {
        let mut data = VecDeque::new();
        let mut data_bytes = 0;
        assert!(enqueue_data(&mut data, &mut data_bytes, &[1; MAX_PENDING_DATA_BYTES]).is_ok());
        assert!(enqueue_data(&mut data, &mut data_bytes, &[2]).is_err());
        assert_eq!(data_bytes, MAX_PENDING_DATA_BYTES);

        let mut media = VecDeque::new();
        let mut media_bytes = 0;
        enqueue_media(&mut media, &mut media_bytes, Tag::Video, &[1; 1536 * 1024]);
        enqueue_media(&mut media, &mut media_bytes, Tag::Video, &[2; 1536 * 1024]);
        enqueue_media(&mut media, &mut media_bytes, Tag::Video, &[3; 1536 * 1024]);
        assert!(media_bytes <= MAX_PENDING_MEDIA_BYTES);
        assert_eq!(media.front().map(|(_, body)| body[0]), Some(2));
    }

    #[tokio::test]
    async fn preauth_and_oversized_frames_are_dropped_but_normal_frames_deliver() {
        let (left, right) = tokio::io::duplex(8 * 1024 * 1024);
        let left = WebSocketStream::from_raw_socket(left, Role::Client, None).await;
        let right = WebSocketStream::from_raw_socket(right, Role::Server, None).await;
        let a = ReliableChannel::spawn(left);
        let mut b = ReliableChannel::spawn(right);
        let mut media_rx = b.take_media_rx().unwrap();

        a.send_frame(Tag::Video, b"preauth").await.unwrap();
        assert!(timeout(Duration::from_millis(100), media_rx.recv()).await.is_err());
        b.enable_application_traffic();
        a.send_frame(Tag::Video, &vec![0; MAX_VIDEO_BODY + 1]).await.unwrap();
        a.send_frame(Tag::Video, b"normal").await.unwrap();
        assert_eq!(timeout(Duration::from_secs(1), media_rx.recv()).await.unwrap(), Some((Tag::Video, b"normal".to_vec())));
    }
}
