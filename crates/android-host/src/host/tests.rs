use super::*;
use crate::input::{Action, Global};
use host_core::capture::ScreenCapturer;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

/// Collects every event the loop emits.
#[derive(Default)]
struct Collector(StdMutex<Vec<HostEvent>>);

impl HostEvents for Collector {
    fn on(&self, event: HostEvent) {
        self.0.lock().unwrap().push(event);
    }
}

impl Collector {
    fn events(&self) -> Vec<HostEvent> {
        self.0.lock().unwrap().clone()
    }
}

fn config() -> HostConfig {
    HostConfig {
        // An address nothing listens on, so registration keeps failing.
        relay_urls: vec!["ws://127.0.0.1:1/ws".into()],
        id: "123456789".into(),
        token: "de".repeat(32),
        password: "secret".into(),
        unattended: false,
    }
}

#[test]
fn events_serialize_to_json() {
    assert_eq!(HostEvent::Status(HostStatus::Online).to_json(), r#"{"t":"status","s":"online"}"#);
    assert_eq!(HostEvent::Status(HostStatus::InSession).to_json(), r#"{"t":"status","s":"in_session"}"#);
    assert_eq!(
        HostEvent::Request { peer_id: "123456789".into() }.to_json(),
        r#"{"t":"request","peer":"123456789"}"#
    );
    assert_eq!(
        HostEvent::SessionStarted { peer_id: "1".into() }.to_json(),
        r#"{"t":"session_started","peer":"1"}"#
    );
    assert_eq!(HostEvent::SessionEnded.to_json(), r#"{"t":"session_ended"}"#);
    assert_eq!(HostEvent::IdentityInUse.to_json(), r#"{"t":"identity_in_use"}"#);
    assert_eq!(
        HostEvent::Error("oh \"no\"\n".into()).to_json(),
        r#"{"t":"error","message":"oh \"no\"\n"}"#
    );
}

#[test]
fn answer_is_idempotent_and_only_fires_once() {
    let (controller, _run) = host(config(), Arc::new(Collector::default()));
    // No prompt waiting: a no-op.
    controller.answer(true);
    let (tx, mut rx) = oneshot::channel();
    *controller.pending.lock().unwrap() = Some(tx);
    controller.answer(true);
    controller.answer(false);
    assert_eq!(rx.try_recv(), Ok(true));
}

#[tokio::test(start_paused = true)]
async fn stop_before_connecting_ends_the_loop_cleanly() {
    let collector = Arc::new(Collector::default());
    let (controller, run) = host(config(), collector.clone());
    let loop_task = tokio::spawn(run);
    // Let the loop reach its first registration attempt and emit Connecting.
    tokio::time::sleep(Duration::from_millis(1)).await;
    controller.stop();
    tokio::time::timeout(Duration::from_secs(5), loop_task)
        .await
        .expect("the loop stops promptly")
        .expect("no panic");
    assert_eq!(collector.events().first(), Some(&HostEvent::Status(HostStatus::Connecting)));
}

#[test]
fn frames_and_actions_handles_share_one_queue() {
    let (controller, _run) = host(config(), Arc::new(Collector::default()));
    let actions = controller.actions();
    controller.actions().push(Action::Global(Global::Home));
    assert_eq!(actions.len(), 1);
    // The frame pusher and the capturer it hands out share a slot.
    controller.frames().push(1, 1, &[1, 2, 3, 255], 0);
    assert!(controller.frames().capturer().next_frame().is_some());
}
