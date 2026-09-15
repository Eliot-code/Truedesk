//! Relay-backed WebSocket transport.

use super::{Transport, TransportError, TransportReceiver, TransportSender};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use protocol::{frame, Tag};
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::Mutex;
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};

/// A transport over an already connected and paired relay WebSocket.
pub struct RelayTransport<S = tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    ws: Arc<Mutex<WebSocketStream<S>>>,
}

pub struct RelaySender<S> {
    sink: SplitSink<WebSocketStream<S>, Message>,
}

pub struct RelayReceiver<S> {
    stream: SplitStream<WebSocketStream<S>>,
}

impl<S> RelayTransport<S> {
    /// Wrap an already connected and paired WebSocket.
    pub fn new(ws: WebSocketStream<S>) -> Self {
        Self {
            ws: Arc::new(Mutex::new(ws)),
        }
    }

    /// Wrap a WebSocket shared with the ICE signaling adapter.
    pub(crate) fn shared_socket(&self) -> Arc<Mutex<WebSocketStream<S>>> {
        Arc::clone(&self.ws)
    }
}

fn wrap_message(tag: Tag, sealed: &[u8]) -> Message {
    Message::Binary(frame::wrap(tag, sealed))
}

fn unwrap_message(message: &[u8]) -> Option<(Tag, Vec<u8>)> {
    let (tag, body) = frame::unwrap(message)?;
    Some((tag, body.to_vec()))
}

#[async_trait::async_trait]
impl<S> Transport for RelayTransport<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    fn into_split(self) -> (Box<dyn TransportSender>, Box<dyn TransportReceiver>) {
        // RelayIceSignaling holds the only temporary clone while ICE runs.
        // establish() consumes that signaling value before returning, so the
        // transport can reclaim the WebSocket and own its split halves.
        let ws = match Arc::try_unwrap(self.ws) {
            Ok(mutex) => mutex.into_inner(),
            Err(_) => panic!("relay WebSocket is still shared after ICE establishment"),
        };
        let (sink, stream) = ws.split();
        (
            Box::new(RelaySender { sink }),
            Box::new(RelayReceiver { stream }),
        )
    }
}

#[async_trait::async_trait]
impl<S> TransportSender for RelaySender<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    async fn send(&mut self, tag: Tag, sealed: &[u8]) -> Result<(), TransportError> {
        self.sink
            .send(wrap_message(tag, sealed))
            .await
            .map_err(|error| TransportError::Io(error.to_string()))
    }
}

#[async_trait::async_trait]
impl<S> TransportReceiver for RelayReceiver<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    async fn recv(&mut self) -> Option<(Tag, Vec<u8>)> {
        while let Some(message) = self.stream.next().await {
            match message {
                Ok(Message::Binary(bytes)) => {
                    if let Some(frame) = unwrap_message(&bytes) {
                        return Some(frame);
                    }
                }
                Ok(_) => {}
                Err(_) => return None,
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{timeout, Duration};
    use tokio_tungstenite::{tungstenite::protocol::Role, WebSocketStream};

    #[test]
    fn transport_message_round_trip_preserves_tag_and_sealed_bytes() {
        let sealed = b"sealed blob";
        let message = wrap_message(Tag::Input, sealed);
        let Message::Binary(bytes) = message else {
            panic!("transport frames must use binary WebSocket messages");
        };

        assert_eq!(unwrap_message(&bytes), Some((Tag::Input, sealed.to_vec())));
    }

    #[tokio::test]
    async fn split_halves_send_while_receive_is_pending() {
        let (left, right) = tokio::io::duplex(64 * 1024);
        let left = WebSocketStream::from_raw_socket(left, Role::Client, None).await;
        let right = WebSocketStream::from_raw_socket(right, Role::Server, None).await;
        let (mut left_tx, mut left_rx) = RelayTransport::new(left).into_split();
        let (mut right_tx, mut right_rx) = RelayTransport::new(right).into_split();

        let left_received = tokio::spawn(async move {
            let mut frames = Vec::new();
            for _ in 0..4 {
                frames.push(left_rx.recv().await.expect("left peer closed"));
            }
            frames
        });
        let right_received = tokio::spawn(async move {
            let mut frames = Vec::new();
            for _ in 0..4 {
                frames.push(right_rx.recv().await.expect("right peer closed"));
            }
            frames
        });

        let sends = async {
            for index in 0..4u8 {
                left_tx.send(Tag::Video, &[index]).await?;
                right_tx.send(Tag::Input, &[index + 10]).await?;
            }
            Ok::<_, TransportError>(())
        };
        timeout(Duration::from_secs(1), sends)
            .await
            .expect("send must not wait for a receive")
            .expect("relay send failed");

        let left_frames = left_received.await.unwrap();
        let right_frames = right_received.await.unwrap();
        assert_eq!(
            left_frames
                .iter()
                .map(|(_, body)| body[0])
                .collect::<Vec<_>>(),
            [10, 11, 12, 13]
        );
        assert_eq!(
            right_frames
                .iter()
                .map(|(_, body)| body[0])
                .collect::<Vec<_>>(),
            [0, 1, 2, 3]
        );
    }
}
