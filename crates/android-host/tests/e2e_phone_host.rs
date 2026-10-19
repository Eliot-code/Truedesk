//! The phone host end to end over a real relay: the host loop registers,
//! a viewer pairs and is allowed in, the phone's pushed frames reach the
//! viewer as encoded video, and the viewer's mouse and keyboard arrive as
//! touch actions.

use android_host::input::Action;
use android_host::{host, HostConfig, HostEvent, HostEvents};
use host_core::relay_client;
use host_core::roles::run_viewer_session;
use protocol::input::InputEvent;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::{sleep, timeout};

const PEER_ID: &str = "444444444";
const VIEWER_ID: &str = "555555555";
const PASSWORD: &str = "phone-pw";

/// Forwards host events to an unbounded channel the test awaits.
struct Events(mpsc::UnboundedSender<HostEvent>);

impl HostEvents for Events {
    fn on(&self, event: HostEvent) {
        let _ = self.0.send(event);
    }
}

/// A 64x64 frame whose single moving pixel changes each call, so the encoder
/// never decides nothing moved.
fn frame(tick: u8) -> Vec<u8> {
    let mut rgba = vec![0u8; 64 * 64 * 4];
    let at = (usize::from(tick) % (64 * 64)) * 4;
    rgba[at..at + 4].copy_from_slice(&[tick, 128, 255, 255]);
    rgba
}

fn tap(x: i32, y: i32, ts: u64) -> Vec<Vec<u8>> {
    vec![
        InputEvent::MouseMove { x, y }.to_bytes(ts),
        InputEvent::MouseButton { button: 1, down: true }.to_bytes(ts + 1),
        InputEvent::MouseButton { button: 1, down: false }.to_bytes(ts + 2),
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_viewer_sees_the_phone_and_its_input_becomes_touch_actions() {
    let addr = relay::spawn_test_server().await;

    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let config = HostConfig {
        relay_urls: vec![format!("ws://{addr}/ws")],
        id: PEER_ID.into(),
        token: "00".repeat(32),
        password: PASSWORD.into(),
        unattended: false,
    };
    let (controller, run) = host(config, Arc::new(Events(event_tx)));
    let host_loop = tokio::spawn(run);

    // Keep pushing the screen so the host always has a fresh frame.
    let pusher = controller.frames();
    let feeding = Arc::new(Mutex::new(true));
    let feeder = {
        let feeding = Arc::clone(&feeding);
        tokio::spawn(async move {
            let mut tick: u8 = 0;
            while *feeding.lock().unwrap() {
                pusher.push(64, 64, &frame(tick), 0);
                tick = tick.wrapping_add(1);
                sleep(Duration::from_millis(30)).await;
            }
        })
    };

    // The viewer connects once the host has had a moment to register.
    sleep(Duration::from_millis(300)).await;
    let mut viewer_ws = relay_client::connect(&format!("ws://{addr}/ws")).await.expect("relay connect");
    relay_client::connect_to(&mut viewer_ws, PEER_ID, VIEWER_ID).await.expect("connect to host");
    let (frame_tx, mut frame_rx) = mpsc::channel(4);
    let (input_tx, input_rx) = mpsc::channel(16);
    let viewer = tokio::spawn(run_viewer_session(viewer_ws, PASSWORD, "s1", frame_tx, input_rx));

    // The host asks whether to allow the viewer; say yes.
    let request = timeout(Duration::from_secs(10), async {
        loop {
            match event_rx.recv().await.expect("event stream open") {
                HostEvent::Request { peer_id } => return peer_id,
                HostEvent::Error(error) => panic!("host error before the prompt: {error}"),
                _ => {}
            }
        }
    })
    .await
    .expect("the host prompts to allow the viewer");
    assert_eq!(request, VIEWER_ID);
    controller.answer(true);

    // A sealed, decoded video frame reaches the viewer: 64x64 H.264.
    let video = timeout(Duration::from_secs(20), frame_rx.recv())
        .await
        .expect("timed out waiting for video")
        .expect("viewer frame sink closed");
    assert!(video.len() > 18, "frame carries an H.264 payload");
    assert_eq!(video[12], 1, "frame declares H.264");
    assert_eq!(&video[14..16], &64u16.to_be_bytes());
    assert_eq!(&video[16..18], &64u16.to_be_bytes());

    // The viewer taps the centre and types 'a'; both cross to the phone.
    for bytes in tap(32_768, 32_768, 100) {
        input_tx.send(bytes).await.expect("send tap");
    }
    input_tx
        .send(InputEvent::Char { ch: 'a', down: true, modifiers: 0 }.to_bytes(200))
        .await
        .expect("send key");

    let actions = controller.actions();
    let mut stroke = false;
    let mut typed = false;
    timeout(Duration::from_secs(10), async {
        while !(stroke && typed) {
            match actions.next(Duration::from_millis(200)) {
                Some(Action::Stroke { points, .. }) => {
                    assert_eq!(points.len(), 1, "a tap is one point");
                    stroke = true;
                }
                Some(Action::Text(text)) => {
                    assert_eq!(text, "a");
                    typed = true;
                }
                Some(other) => panic!("unexpected action {other:?}"),
                None => sleep(Duration::from_millis(20)).await,
            }
        }
    })
    .await
    .expect("the viewer's input became touch actions");
    assert!(stroke && typed);

    // The session-started event names the viewer.
    let started = std::iter::from_fn(|| event_rx.try_recv().ok())
        .any(|event| event == HostEvent::SessionStarted { peer_id: VIEWER_ID.into() });
    assert!(started, "a session-started event was emitted");

    *feeding.lock().unwrap() = false;
    controller.stop();
    let _ = feeder.await;
    viewer.abort();
    host_loop.abort();
}
