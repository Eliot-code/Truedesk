// Spawns the relay, connects a host and a viewer as WS clients,
// registers/connects, then asserts a binary frame from host reaches viewer
// unchanged and the relay never altered it.
use futures_util::{SinkExt, StreamExt};
use protocol::{control::Control, frame, Tag};
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn host_frame_reaches_viewer_unchanged() {
    let addr = relay::spawn_test_server().await; // returns e.g. "127.0.0.1:PORT"
    let url = format!("ws://{addr}/ws");

    let (mut host, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let (mut viewer, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

    host.send(Message::Binary(frame::wrap(
        Tag::Control,
        &Control::Register { peer_id: "abc".into(), ver: 1 }.to_bytes(),
    ))).await.unwrap();

    // Avoid the connect racing ahead of the register (no wait-queue yet).
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    viewer.send(Message::Binary(frame::wrap(
        Tag::Control,
        &Control::Connect { target: "abc".into(), ver: 1 }.to_bytes(),
    ))).await.unwrap();

    // drain the `paired` control message on the viewer side
    let _paired = viewer.next().await.unwrap().unwrap();

    let payload = frame::wrap(Tag::Video, b"opaque-sealed-bytes");
    host.send(Message::Binary(payload.clone())).await.unwrap();

    let got = viewer.next().await.unwrap().unwrap();
    assert_eq!(got.into_data(), payload);
}
