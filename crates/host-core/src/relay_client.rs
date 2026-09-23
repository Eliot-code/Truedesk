//! Small helpers for the relay's existing register/connect control frames.

use futures_util::SinkExt;
use protocol::{control::Control, frame, Tag};
use tokio::net::TcpStream;
use tokio::time::{timeout, Duration};
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

/// The WebSocket type returned by [`connect`].
pub type RelayWebSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Selects relays in configured order and returns the first successful
/// handshake. This is deliberately deterministic; ordering lets users keep a
/// preferred primary while still failing over quickly to later regions.
pub async fn connect_best(urls: &[String]) -> Result<(String, RelayWebSocket), String> {
    let mut last_error = "no relay URLs configured".to_owned();
    for url in urls.iter().filter(|url| !url.trim().is_empty()) {
        match timeout(Duration::from_secs(3), connect(url)).await {
            Ok(Ok(ws)) => return Ok((url.clone(), ws)),
            Ok(Err(error)) => last_error = error.to_string(),
            Err(_) => last_error = format!("relay connection timed out: {url}"),
        }
    }
    Err(last_error)
}

pub async fn select_first_reachable<F, Fut>(urls: &[String], mut connector: F) -> Result<String, String>
where
    F: FnMut(String) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    for url in urls {
        if connector(url.clone()).await.is_ok() {
            return Ok(url.clone());
        }
    }
    Err("no relay reachable".into())
}

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

#[cfg(test)]
mod tests {
    use super::select_first_reachable;

    #[tokio::test]
    async fn selection_returns_first_reachable_url_in_order() {
        let urls = vec!["wss://eu".into(), "wss://us".into(), "wss://apac".into()];
        let selected = select_first_reachable(&urls, |url| async move {
            if url == "wss://us" { Ok(()) } else { Err("offline".into()) }
        }).await.unwrap();
        assert_eq!(selected, "wss://us");
    }
}
