use protocol::{crypto::{Opener, Sealer}, Tag};
use protocol::input::InputEvent;
use protocol::kex::derive_keys;

#[test]
fn input_event_roundtrips_viewer_to_host() {
    let shared = [7u8; 32];
    let (v_send, _v_recv) = derive_keys(&shared, false); // viewer send = v2h
    let (_h_send, h_recv) = derive_keys(&shared, true); // host recv = v2h
    let mut sealer = Sealer::new(v_send);
    let mut opener = Opener::new(h_recv);
    let ev = InputEvent::Key {
        code: 65,
        down: true,
        modifiers: 0,
    };
    let plaintext = ev.to_bytes(42);
    let sealed = sealer.seal(&[Tag::Input as u8], &plaintext);
    let opened = opener.open(&[Tag::Input as u8], &sealed).expect("host opens viewer input");
    assert_eq!(opened, plaintext);
    assert_eq!(InputEvent::from_bytes(&opened), Some((ev, 42)));
}
