//! Relay-backed WebSocket transport.

use super::{Transport, TransportError};
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
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    async fn send(&mut self, tag: Tag, sealed: &[u8]) -> Result<(), TransportError> {
        self.ws
            .lock()
            .await
            .send(wrap_message(tag, sealed))
            .await
            .map_err(|error| TransportError::Io(error.to_string()))
    }

    async fn recv(&mut self) -> Option<(Tag, Vec<u8>)> {
        let mut ws = self.ws.lock().await;
        while let Some(message) = ws.next().await {
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

    #[test]
    fn transport_message_round_trip_preserves_tag_and_sealed_bytes() {
        let sealed = b"sealed blob";
        let message = wrap_message(Tag::Input, sealed);
        let Message::Binary(bytes) = message else {
            panic!("transport frames must use binary WebSocket messages");
        };

        assert_eq!(unwrap_message(&bytes), Some((Tag::Input, sealed.to_vec())));
    }
}
