use host_core::{
    capture::FakeCapturer,
    encode::H264Encoder,
    inject::InputInjector,
    relay_client,
    roles::{run_host_session, run_viewer_session},
};
use protocol::input::InputEvent;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::time::{sleep, timeout, Duration};

const PEER_ID: &str = "111111111";
const PASSWORD: &str = "testpw";

struct RecordingInjector(Arc<AtomicUsize>);

impl InputInjector for RecordingInjector {
    fn inject(&mut self, _event: &InputEvent, _screen_w: u32, _screen_h: u32) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

async fn connect_to_peer(addr: &str, viewer: bool) -> relay_client::RelayWebSocket {
    let ws_url = format!("ws://{addr}/ws");
    let mut ws = relay_client::connect(&ws_url).await.expect("relay connect");
    if viewer {
        relay_client::connect_to(&mut ws, PEER_ID)
            .await
            .expect("relay connect_to");
    } else {
        relay_client::register(&mut ws, PEER_ID)
            .await
            .expect("relay register");
    }
    ws
}

async fn wait_for_count(count: &AtomicUsize) {
    timeout(Duration::from_secs(5), async {
        while count.load(Ordering::SeqCst) == 0 {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("host did not inject the viewer input");
}

async fn run_positive_session() {
    let addr = relay::spawn_test_server().await;
    let injector_count = Arc::new(AtomicUsize::new(0));
    let host_injector_count = Arc::clone(&injector_count);

    let host_ws = connect_to_peer(&addr, false).await;
    let host = tokio::spawn(async move {
        run_host_session(
            host_ws,
            PASSWORD,
            "e2e-positive",
            FakeCapturer::new(64, 64),
            |width, height| H264Encoder::new(width, height, 15, 1_000_000),
            RecordingInjector(host_injector_count),
        )
        .await
    });

    let viewer_ws = connect_to_peer(&addr, true).await;
    let (frame_tx, mut frame_rx) = tokio::sync::mpsc::channel(4);
    let (input_tx, input_rx) = tokio::sync::mpsc::channel(4);
    let viewer = tokio::spawn(run_viewer_session(
        viewer_ws,
        PASSWORD,
        "e2e-positive",
        frame_tx,
        input_rx,
    ));

    let frame = timeout(Duration::from_secs(15), frame_rx.recv())
        .await
        .expect("timed out waiting for a sealed video frame")
        .expect("viewer frame sink closed");
    assert!(frame.len() > 18, "opened frame must contain H.264 payload");
    assert_eq!(frame[12], 1, "opened frame must declare H.264");
    assert_eq!(&frame[14..16], &64u16.to_be_bytes());
    assert_eq!(&frame[16..18], &64u16.to_be_bytes());

    let input = InputEvent::Key {
        code: 65,
        down: true,
        modifiers: 0,
    }
    .to_bytes(42);
    input_tx.send(input).await.expect("send viewer input");
    wait_for_count(&injector_count).await;

    viewer.abort();
    host.abort();
}

async fn run_wrong_password_session() {
    let addr = relay::spawn_test_server().await;
    let host_ws = connect_to_peer(&addr, false).await;
    let host = tokio::spawn(async move {
        run_host_session(
            host_ws,
            PASSWORD,
            "e2e-negative",
            FakeCapturer::new(64, 64),
            |width, height| H264Encoder::new(width, height, 15, 1_000_000),
            RecordingInjector(Arc::new(AtomicUsize::new(0))),
        )
        .await
    });

    let viewer_ws = connect_to_peer(&addr, true).await;
    let (frame_tx, mut frame_rx) = tokio::sync::mpsc::channel(4);
    let (_input_tx, input_rx) = tokio::sync::mpsc::channel(4);
    let viewer = tokio::spawn(run_viewer_session(
        viewer_ws,
        "wrong-password",
        "e2e-negative",
        frame_tx,
        input_rx,
    ));

    let result = timeout(Duration::from_secs(5), frame_rx.recv()).await;
    assert!(
        !matches!(result, Ok(Some(_))),
        "wrong password must not produce media"
    );
    viewer.abort();
    host.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn relay_host_viewer_session_seals_media_and_input() {
    run_positive_session().await;
    run_wrong_password_session().await;
}
