//! Small helpers for the relay's existing register/connect control frames.

use futures_util::SinkExt;
use protocol::{control::Control, frame, Tag};
use tokio::net::TcpStream;
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

/// The WebSocket type returned by [`connect`].
pub type RelayWebSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Open a WebSocket connection to a relay endpoint.
pub async fn connect(url: &str) -> Result<RelayWebSocket, tokio_tungstenite::tungstenite::Error> {
    Ok(connect_async(url).await?.0)
}

/// Register this peer ID on an already connected relay WebSocket.
pub async fn register(
    ws: &mut RelayWebSocket,
    peer_id: &str,
) -> Result<(), tokio_tungstenite::tungstenite::Error> {
    ws.send(Message::Binary(frame::wrap(
        Tag::Control,
        &Control::Register {
            peer_id: peer_id.into(),
            ver: 1,
        }
        .to_bytes(),
    )))
    .await
}

/// Ask the relay to connect this WebSocket to a registered peer.
pub async fn connect_to(
    ws: &mut RelayWebSocket,
    target: &str,
) -> Result<(), tokio_tungstenite::tungstenite::Error> {
    ws.send(Message::Binary(frame::wrap(
        Tag::Control,
        &Control::Connect {
            target: target.into(),
            ver: 1,
        }
        .to_bytes(),
    )))
    .await
}
