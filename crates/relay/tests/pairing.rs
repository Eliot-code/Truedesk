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

/// A browser's upgrade carries an Origin, offers permessage-deflate and
/// names no subprotocol. The relay takes it like any other client.
#[tokio::test]
async fn browser_style_upgrade_pairs() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let addr = relay::spawn_test_server().await;
    let url = format!("ws://{addr}/ws");
    let (mut host, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    host.send(control(Control::Register { peer_id: "123456789".into(), token: "00".repeat(32), ver: 1 })).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let mut request = url.into_client_request().unwrap();
    request.headers_mut().insert("Origin", "https://treeot.github.io".parse().unwrap());
    request.headers_mut().insert("Sec-WebSocket-Extensions", "permessage-deflate; client_max_window_bits".parse().unwrap());
    let (mut browser, response) = tokio_tungstenite::connect_async(request).await.unwrap();
    assert!(response.headers().get("Sec-WebSocket-Extensions").is_none(), "no compression is negotiated");
    assert!(response.headers().get("Sec-WebSocket-Protocol").is_none());
    browser.send(control(Control::Connect { target: "123456789".into(), peer_id: "987654321".into(), ver: 1 })).await.unwrap();
    let paired = browser.next().await.unwrap().unwrap().into_data();
    assert!(matches!(frame::unwrap(&paired), Some((Tag::Control, body)) if matches!(Control::from_bytes(body), Some(Control::Paired { .. }))));

    let payload = frame::wrap(Tag::Video, b"sealed");
    host.send(Message::Binary(payload.clone().into())).await.unwrap();
    assert_eq!(browser.next().await.unwrap().unwrap().into_data(), payload);
}

type Client = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn register(url: &str, token: &str) -> Client {
    let (mut host, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    host.send(control(Control::Register { peer_id: "123456789".into(), token: token.into(), ver: 1 })).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    host
}

async fn first_control(ws: &mut Client) -> Option<Control> {
    let reply = tokio::time::timeout(std::time::Duration::from_secs(2), ws.next()).await.ok()??.ok()?;
    let data = reply.into_data();
    frame::unwrap(&data).and_then(|(tag, body)| if tag == Tag::Control { Control::from_bytes(body) } else { None })
}

async fn viewer_connect(url: &str) -> Client {
    let (mut viewer, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    viewer.send(control(Control::Connect { target: "123456789".into(), peer_id: "987654321".into(), ver: 1 })).await.unwrap();
    viewer
}

#[tokio::test]
async fn a_parked_host_that_disconnects_is_unregistered() {
    let addr = relay::spawn_test_server().await;
    let url = format!("ws://{addr}/ws");
    let host = register(&url, &"00".repeat(32)).await;
    drop(host);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let mut viewer = viewer_connect(&url).await;
    assert_eq!(first_control(&mut viewer).await, Some(Control::Error { code: protocol::control::ERROR_PEER_UNAVAILABLE.into() }));
}

#[tokio::test]
async fn data_a_host_sends_before_pairing_reaches_the_viewer() {
    let addr = relay::spawn_test_server().await;
    let url = format!("ws://{addr}/ws");
    let mut host = register(&url, &"00".repeat(32)).await;
    let early = frame::wrap(Tag::Video, b"sent-while-parked");
    host.send(Message::Binary(early.clone().into())).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let mut viewer = viewer_connect(&url).await;
    assert!(matches!(first_control(&mut viewer).await, Some(Control::Paired { .. })));
    let got = tokio::time::timeout(std::time::Duration::from_secs(2), viewer.next()).await.unwrap().unwrap().unwrap();
    assert_eq!(got.into_data(), early);
}

#[tokio::test]
async fn copies_of_one_device_fighting_over_an_id_get_id_in_use() {
    let addr = relay::spawn_test_server().await;
    let url = format!("ws://{addr}/ws");
    let token = "00".repeat(32);
    // Each newcomer displaces the live registration before it, as two
    // running machines with one ID and token do.
    let mut live = register(&url, &token).await;
    for _ in 0..3 { live = register(&url, &token).await; }
    let mut newest = register(&url, &token).await;
    assert_eq!(first_control(&mut newest).await, Some(Control::Error { code: protocol::control::ERROR_ID_IN_USE.into() }));
    // The registration that was live is still the one viewers reach.
    let mut viewer = viewer_connect(&url).await;
    assert!(matches!(first_control(&mut viewer).await, Some(Control::Paired { .. })));
    assert!(matches!(first_control(&mut live).await, Some(Control::Paired { .. })));
}

#[tokio::test]
async fn a_host_that_reconnects_cleanly_is_never_in_use() {
    let addr = relay::spawn_test_server().await;
    let url = format!("ws://{addr}/ws");
    let token = "00".repeat(32);
    for _ in 0..6 {
        let mut host = register(&url, &token).await;
        host.close(None).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let mut host = register(&url, &token).await;
    let mut viewer = viewer_connect(&url).await;
    assert!(matches!(first_control(&mut viewer).await, Some(Control::Paired { .. })));
    assert!(matches!(first_control(&mut host).await, Some(Control::Paired { .. })));
}

#[tokio::test]
async fn every_refusal_says_why() {
    use protocol::control::{ERROR_CONNECT_RATE, ERROR_INVALID_CONNECT, ERROR_INVALID_CONTROL, ERROR_INVALID_REGISTER};
    let addr = relay::spawn_test_server().await;
    let url = format!("ws://{addr}/ws");
    let error = |code: &str| Some(Control::Error { code: code.into() });

    let (mut host, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    host.send(control(Control::Register { peer_id: "123456789".into(), token: "not-a-token".into(), ver: 1 })).await.unwrap();
    assert_eq!(first_control(&mut host).await, error(ERROR_INVALID_REGISTER));

    let (mut viewer, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    viewer.send(control(Control::Connect { target: "12".into(), peer_id: "987654321".into(), ver: 1 })).await.unwrap();
    assert_eq!(first_control(&mut viewer).await, error(ERROR_INVALID_CONNECT));

    let (mut other, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    other.send(control(Control::KeepAlive)).await.unwrap();
    assert_eq!(first_control(&mut other).await, error(ERROR_INVALID_CONTROL));

    // Six connects per ID per 10 s from one address.
    let mut last = None;
    for _ in 0..7 {
        last = first_control(&mut viewer_connect(&url).await).await;
    }
    assert_eq!(last, error(ERROR_CONNECT_RATE));
}

#[tokio::test]
async fn a_full_relay_says_it_is_busy() {
    let addr = relay::spawn_test_server().await;
    let url = format!("ws://{addr}/ws");
    // 20 connection attempts per address per 10 s.
    let mut open = Vec::new();
    for _ in 0..20 {
        open.push(tokio_tungstenite::connect_async(&url).await.unwrap().0);
    }
    let (mut refused, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    assert_eq!(first_control(&mut refused).await, Some(Control::Error { code: protocol::control::ERROR_RELAY_BUSY.into() }));
}

#[tokio::test]
async fn a_parked_host_gets_pongs() {
    let addr = relay::spawn_test_server().await;
    let mut host = register(&format!("ws://{addr}/ws"), &"00".repeat(32)).await;
    host.send(Message::Ping(b"x".to_vec().into())).await.unwrap();
    let reply = tokio::time::timeout(std::time::Duration::from_secs(2), host.next()).await.unwrap().unwrap().unwrap();
    assert_eq!(reply, Message::Pong(b"x".to_vec().into()));
}
