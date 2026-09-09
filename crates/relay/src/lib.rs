mod registry;
mod session;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use registry::{Outbound, Registry};
use session::Bridge;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

pub fn app() -> Router {
    Router::new()
        .route("/ws", get(ws_handler))
        .with_state(Registry::default())
}

/// Test helper: bind an ephemeral port and return its address.
pub async fn spawn_test_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move { axum::serve(listener, app()).await.unwrap() });
    addr
}

async fn ws_handler(ws: WebSocketUpgrade, State(reg): State<Registry>) -> axum::response::Response {
    ws.on_upgrade(move |socket| handle(socket, reg))
}

async fn handle(socket: WebSocket, reg: Registry) {
    let (mut sink, mut stream) = socket.split();
    let (my_tx, mut my_rx) = mpsc::unbounded_channel::<Vec<u8>>();

    // writer task: anything queued to my_tx gets written to this socket
    let writer = tokio::spawn(async move {
        while let Some(bytes) = my_rx.recv().await {
            if sink.send(Message::Binary(bytes)).await.is_err() {
                break;
            }
        }
    });

    // first message must be a Control (register or connect)
    let first = match stream.next().await {
        Some(Ok(Message::Binary(b))) => b,
        _ => {
            writer.abort();
            return;
        }
    };

    let peer_tx: Outbound = match session::classify(&first, my_tx.clone(), &reg) {
        Some(Bridge::ViewerPaired(host_tx)) => {
            // notify both sides they are paired
            let _ = host_tx.send(session::paired_frame("host"));
            let _ = my_tx.send(session::paired_frame("viewer"));
            host_tx
        }
        Some(Bridge::HostRegistered(notify_rx)) => {
            // Parked as a host: wait for a viewer's `connect` to hand us its
            // sender, then forward our traffic there.
            match notify_rx.await {
                Ok(viewer_tx) => viewer_tx,
                Err(_) => {
                    writer.abort();
                    return;
                }
            }
        }
        None => {
            writer.abort();
            return;
        }
    };

    forward_rest(stream, Some(peer_tx)).await;
    writer.abort();
}

async fn forward_rest(
    mut stream: futures_util::stream::SplitStream<WebSocket>,
    peer_tx: Option<Outbound>,
) {
    while let Some(Ok(msg)) = stream.next().await {
        if let Message::Binary(bytes) = msg {
            if let Some(ref tx) = peer_tx {
                if tx.send(bytes).is_err() {
                    break;
                }
            }
        }
    }
}
