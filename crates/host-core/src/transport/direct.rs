//! Direct UDP transport over a nominated ICE connection.

use super::{Transport, TransportError};
use protocol::{frame, Tag};
use std::sync::Arc;
use webrtc_util::conn::Conn;

/// Carries sealed protocol frames as single UDP datagrams.
pub struct DirectTransport {
    pub(crate) conn: Arc<dyn Conn + Send + Sync>,
}

impl DirectTransport {
    pub(crate) fn new(conn: Arc<dyn Conn + Send + Sync>) -> Self {
        Self { conn }
    }
}

#[async_trait::async_trait]
impl Transport for DirectTransport {
    async fn send(&mut self, tag: Tag, sealed: &[u8]) -> Result<(), TransportError> {
        let datagram = frame::wrap(tag, sealed);
        let sent = self
            .conn
            .send(&datagram)
            .await
            .map_err(|error| TransportError::Io(error.to_string()))?;
        if sent == datagram.len() {
            Ok(())
        } else {
            Err(TransportError::Io(format!(
                "UDP send was incomplete: sent {sent} of {} bytes",
                datagram.len()
            )))
        }
    }

    async fn recv(&mut self) -> Option<(Tag, Vec<u8>)> {
        // The maximum IPv4/IPv6 UDP payload is below this size. Frames larger
        // than the path MTU may still be dropped by the network; this
        // transport deliberately does not implement fragmentation yet.
        let mut buf = vec![0u8; 65_535];
        let size = self.conn.recv(&mut buf).await.ok()?;
        let (tag, body) = frame::unwrap(&buf[..size])?;
        Some((tag, body.to_vec()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use webrtc_util::conn::conn_pipe::pipe;

    #[tokio::test]
    async fn loopback_round_trip_preserves_tag_and_sealed_bytes() {
        let (left, right) = pipe();

        let mut sender = DirectTransport::new(Arc::new(left));
        let mut receiver = DirectTransport::new(Arc::new(right));
        let sealed = b"already sealed bytes";

        sender.send(Tag::Video, sealed).await.unwrap();
        assert_eq!(receiver.recv().await, Some((Tag::Video, sealed.to_vec())));
    }
}
