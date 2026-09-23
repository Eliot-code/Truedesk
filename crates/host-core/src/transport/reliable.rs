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
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};

const CHANNEL_CAPACITY: usize = 128;
/// Upper bound on Data frames buffered while the consumer is slow/absent, so a
/// peer flooding Tag::Data cannot grow memory without limit. On overflow the
/// oldest pending Data frame is dropped.
const MAX_PENDING_DATA: usize = 256;

/// A reliable tagged channel over a relay WebSocket.
pub struct ReliableChannel<S> {
    sender: ReliableSender,
    data_rx: Option<mpsc::Receiver<Vec<u8>>>,
    media_rx: Option<mpsc::Receiver<(Tag, Vec<u8>)>>,
    control_rx: Option<mpsc::Receiver<Vec<u8>>>,
    reader_task: JoinHandle<()>,
    writer_task: JoinHandle<()>,
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
        let (data_tx, data_rx) = mpsc::channel(CHANNEL_CAPACITY);
        let (media_tx, media_rx) = mpsc::channel(CHANNEL_CAPACITY);
        let (control_tx, control_rx) = mpsc::channel(CHANNEL_CAPACITY);

        let writer_task = tokio::spawn(async move {
            let mut sink = sink;
            while let Some(bytes) = writer_rx.recv().await {
                if sink.send(Message::Binary(bytes)).await.is_err() {
                    break;
                }
            }
        });

        let reader_task = tokio::spawn(async move {
            let mut pending_data = VecDeque::new();
            loop {
                let message = if let Some(body) = pending_data.front().cloned() {
                    tokio::select! {
                        result = data_tx.send(body) => {
                            if result.is_err() { break; }
                            pending_data.pop_front();
                            continue;
                        }
                        message = stream.next() => message,
                    }
                } else {
                    stream.next().await
                };
                let Some(message) = message else { break };
                let Ok(Message::Binary(bytes)) = message else {
                    break;
                };
                let Some((tag, body)) = frame::unwrap(&bytes) else {
                    continue;
                };

                match tag {
                    // Data is reliable and ordered. A full data queue applies
                    // backpressure to Data delivery via the select! above, while
                    // the demux keeps reading media so a slow Data consumer
                    // cannot starve it. The pending buffer is bounded so a peer
                    // flooding Data at a wedged/absent consumer cannot grow
                    // memory without limit: on overflow the oldest Data frame is
                    // dropped, which a higher layer detects (e.g. file transfer
                    // fails its hash check) rather than the process OOMing.
                    Tag::Data => {
                        if pending_data.len() >= MAX_PENDING_DATA {
                            pending_data.pop_front();
                        }
                        pending_data.push_back(body.to_vec());
                    }
                    // Media is lossy under pressure. Dropping the newest item
                    // keeps a slow media consumer from stopping the demux.
                    Tag::Video | Tag::Audio | Tag::Input => {
                        let item = (tag, body.to_vec());
                        let _ = media_tx.try_send(item);
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
            _stream: std::marker::PhantomData,
        }
    }

    /// Queue a tagged frame for the single WebSocket writer.
    pub async fn send_frame(&self, tag: Tag, body: &[u8]) -> Result<(), TransportError> {
        self.sender.send_frame(tag, body).await
    }

    pub fn sender(&self) -> ReliableSender { self.sender.clone() }

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

        let data = data_reader.await.unwrap();
        let media = media_reader.await.unwrap();
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
}
