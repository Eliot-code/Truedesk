// Spawns the relay, connects a host and a viewer as WS clients,
// registers/connects, then asserts a binary frame from host reaches viewer
// unchanged and the relay never altered it.
use futures_util::{SinkExt, StreamExt};
use protocol::{control::Control, frame, Tag};
use tokio_tungstenite::tungstenite::Message;

async fn active_connections(addr: &str) -> usize {
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    stream.write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    response.split("\"active_connections\":").nth(1).and_then(|value| value.split('}').next()).unwrap().parse().unwrap()
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
    ))).await.unwrap();

    // Avoid the connect racing ahead of the register (no wait-queue yet).
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    viewer.send(Message::Binary(frame::wrap(
        Tag::Control,
        &Control::Connect { target: "123456789".into(), peer_id: "987654321".into(), ver: 1 }.to_bytes(),
    ))).await.unwrap();

    // drain the `paired` control message on the viewer side
    let _paired = viewer.next().await.unwrap().unwrap();

    let payload = frame::wrap(Tag::Video, b"opaque-sealed-bytes");
    host.send(Message::Binary(payload.clone())).await.unwrap();

    let got = viewer.next().await.unwrap().unwrap();
    assert_eq!(got.into_data(), payload);
}

#[tokio::test]
async fn dropping_host_closes_viewer_and_releases_connections() {
    let addr = relay::spawn_test_server().await;
    let baseline = active_connections(&addr).await;
    let url = format!("ws://{addr}/ws");
    let (mut host, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let (mut viewer, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    host.send(Message::Binary(frame::wrap(Tag::Control, &Control::Register { peer_id: "123456789".into(), token: "00".repeat(32), ver: 1 }.to_bytes()))).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    viewer.send(Message::Binary(frame::wrap(Tag::Control, &Control::Connect { target: "123456789".into(), peer_id: "987654321".into(), ver: 1 }.to_bytes()))).await.unwrap();
    let _ = viewer.next().await.unwrap().unwrap();
    drop(host);
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), viewer.next()).await;
    assert!(matches!(result, Ok(None) | Ok(Some(Err(_))) | Ok(Some(Ok(Message::Close(_))))), "viewer remained open: {result:?}");
    let released = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if active_connections(&addr).await == baseline { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await;
    assert!(released.is_ok(), "relay connections did not return to baseline");
}
