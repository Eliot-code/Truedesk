//! Small helpers for the relay's existing register/connect control frames.

// These return tungstenite's own error type unchanged so callers can classify
// it (see reconnect.rs); boxing it would only add an allocation per failure.
#![allow(clippy::result_large_err)]

use futures_util::{SinkExt, StreamExt};
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
    token: &str,
) -> Result<(), tokio_tungstenite::tungstenite::Error> {
    ws.send(Message::Binary(frame::wrap(
        Tag::Control,
        &Control::Register {
            peer_id: peer_id.into(),
            token: token.into(),
            ver: 1,
        }
        .to_bytes(),
    )))
    .await
}

/// A host registration that a viewer has paired with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pairing {
    pub session: String,
    pub peer_id: String,
}

/// Connect to the first reachable relay and register `peer_id` with `token`.
/// The relay does not acknowledge registration; a rejection surfaces as the
/// socket closing in [`await_pairing`].
pub async fn register_host(urls: &[String], peer_id: &str, token: &str) -> Result<RelayWebSocket, String> {
    let (_, mut ws) = connect_best(urls).await?;
    register(&mut ws, peer_id, token).await.map_err(|error| error.to_string())?;
    Ok(ws)
}

/// Wait on a registered host socket until the relay pairs a viewer with it.
/// Errors when the relay closes the registration (rejected, pairing timeout,
/// relay restart) or sends something other than a `paired` frame.
pub async fn await_pairing(ws: &mut RelayWebSocket) -> Result<Pairing, String> {
    loop {
        let message = match ws.next().await {
            Some(Ok(Message::Binary(message))) => message,
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
            _ => return Err("relay closed while hosting".into()),
        };
        let Some((Tag::Control, body)) = frame::unwrap(&message) else {
            return Err("invalid pairing message".into());
        };
        let Some(Control::Paired { session, peer_id, .. }) = Control::from_bytes(body) else {
            return Err("invalid pairing message".into());
        };
        return Ok(Pairing { session, peer_id });
    }
}

/// Ask the relay to connect this WebSocket to a registered peer.
pub async fn connect_to(
    ws: &mut RelayWebSocket,
    target: &str,
    peer_id: &str,
) -> Result<(), tokio_tungstenite::tungstenite::Error> {
    ws.send(Message::Binary(frame::wrap(
        Tag::Control,
        &Control::Connect {
            target: target.into(),
            peer_id: peer_id.into(),
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
