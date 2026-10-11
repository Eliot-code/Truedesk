// Spawns the relay, connects a host and a viewer as WS clients,
// registers/connects, then asserts a binary frame from host reaches viewer
// unchanged and the relay never altered it.
use futures_util::{SinkExt, StreamExt};
use protocol::{control::Control, frame, Tag};
use tokio_tungstenite::tungstenite::Message;

async fn health_body(addr: &str) -> String {
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    stream.write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    response.split("\r\n\r\n").nth(1).unwrap().to_owned()
}

#[tokio::test]
async fn host_frame_reaches_viewer_unchanged() {
    let addr = relay::spawn_test_server().await; // returns e.g. "127.0.0.1:PORT"
    let url = format!("ws://{addr}/ws");

    let (mut host, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let (mut viewer, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

    host.send(Message::Binary(frame::wrap(
        Tag::Control,
        &Control::Register { peer_id: "123456789".into(), token: "00".repeat(32), ver: 1 }.to_bytes(),
    ).into())).await.unwrap();

    // Avoid the connect racing ahead of the register (no wait-queue yet).
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    viewer.send(Message::Binary(frame::wrap(
        Tag::Control,
        &Control::Connect { target: "123456789".into(), peer_id: "987654321".into(), ver: 1 }.to_bytes(),
    ).into())).await.unwrap();

    // drain the `paired` control message on the viewer side
    let _paired = viewer.next().await.unwrap().unwrap();

    let payload = frame::wrap(Tag::Video, b"opaque-sealed-bytes");
    host.send(Message::Binary(payload.clone().into())).await.unwrap();

    let got = viewer.next().await.unwrap().unwrap();
    assert_eq!(got.into_data(), payload);
}

#[tokio::test]
async fn dropping_host_closes_viewer_and_releases_connections() {
    let (addr, handle) = relay::spawn_test_server_with_handle().await;
    let baseline = handle.active_connections();
    let url = format!("ws://{addr}/ws");
    let (mut host, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let (mut viewer, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    host.send(Message::Binary(frame::wrap(Tag::Control, &Control::Register { peer_id: "123456789".into(), token: "00".repeat(32), ver: 1 }.to_bytes()).into())).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    viewer.send(Message::Binary(frame::wrap(Tag::Control, &Control::Connect { target: "123456789".into(), peer_id: "987654321".into(), ver: 1 }.to_bytes()).into())).await.unwrap();
    let _ = viewer.next().await.unwrap().unwrap();
    drop(host);
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), viewer.next()).await;
    assert!(matches!(result, Ok(None) | Ok(Some(Err(_))) | Ok(Some(Ok(Message::Close(_))))), "viewer remained open: {result:?}");
    let released = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if handle.active_connections() == baseline { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await;
    assert!(released.is_ok(), "relay connections did not return to baseline");
}

fn control(message: Control) -> Message { Message::Binary(frame::wrap(Tag::Control, &message.to_bytes()).into()) }

#[tokio::test]
async fn health_reports_status_only() {
    let addr = relay::spawn_test_server().await;
    assert_eq!(health_body(&addr).await, r#"{"status":"ok"}"#);
}

#[tokio::test]
async fn register_with_foreign_token_gets_id_taken_error() {
    let addr = relay::spawn_test_server().await;
    let url = format!("ws://{addr}/ws");
    let (mut owner, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    owner.send(control(Control::Register { peer_id: "123456789".into(), token: "00".repeat(32), ver: 1 })).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let (mut squatter, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    squatter.send(control(Control::Register { peer_id: "123456789".into(), token: "11".repeat(32), ver: 1 })).await.unwrap();
    let reply = tokio::time::timeout(std::time::Duration::from_secs(2), squatter.next()).await.unwrap().unwrap().unwrap();
    let data = reply.into_data();
    let (tag, body) = frame::unwrap(&data).unwrap();
    assert_eq!(tag, Tag::Control);
    assert_eq!(Control::from_bytes(body), Some(Control::Error { code: protocol::control::ERROR_ID_TAKEN.into() }));
    let close = tokio::time::timeout(std::time::Duration::from_secs(2), squatter.next()).await.unwrap();
    assert!(matches!(close, Some(Ok(Message::Close(_))) | None | Some(Err(_))), "{close:?}");
}

#[tokio::test]
async fn connect_to_absent_peer_gets_peer_unavailable_error() {
    let addr = relay::spawn_test_server().await;
    let (mut viewer, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws")).await.unwrap();
    viewer.send(control(Control::Connect { target: "123456789".into(), peer_id: "987654321".into(), ver: 1 })).await.unwrap();
    let reply = tokio::time::timeout(std::time::Duration::from_secs(2), viewer.next()).await.unwrap().unwrap().unwrap();
    let data = reply.into_data();
    let (tag, body) = frame::unwrap(&data).unwrap();
    assert_eq!(tag, Tag::Control);
    assert_eq!(Control::from_bytes(body), Some(Control::Error { code: protocol::control::ERROR_PEER_UNAVAILABLE.into() }));
}

#[tokio::test]
async fn ice_burst_after_pairing_is_not_rate_limited() {
    let addr = relay::spawn_test_server().await;
    let url = format!("ws://{addr}/ws");
    let (mut host, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let (mut viewer, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    host.send(control(Control::Register { peer_id: "123456789".into(), token: "00".repeat(32), ver: 1 })).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    viewer.send(control(Control::Connect { target: "123456789".into(), peer_id: "987654321".into(), ver: 1 })).await.unwrap();
    let _ = host.next().await.unwrap().unwrap();
    let _ = viewer.next().await.unwrap().unwrap();
    // More candidates than the old 6-frame signaling burst allowed.
    const CANDIDATES: usize = 32;
    for index in 0..CANDIDATES {
        viewer.send(control(Control::Ice { session: "s1".into(), cand: format!("candidate {index}") })).await.unwrap();
    }
    let mut index = 0;
    while index < CANDIDATES {
        let got = tokio::time::timeout(std::time::Duration::from_secs(2), host.next()).await.expect("host starved").unwrap().unwrap();
        let data = got.into_data();
        let (_, body) = frame::unwrap(&data).unwrap();
        // The host may see a second `paired` frame; skip it.
        let Some(Control::Ice { cand, .. }) = Control::from_bytes(body) else { continue };
        assert_eq!(cand, format!("candidate {index}"));
        index += 1;
    }
}
