//! The viewer side of an OpenDesk session, for the browser.
//!
//! A sans-IO state machine: the page owns the WebSocket and the clock, this
//! crate owns pairing, SPAKE2, key confirmation and sealing. It follows
//! `host_core::roles::run_viewer_session` step for step and uses `protocol`
//! for all crypto, so a browser is just another viewer to the host.
//!
//! Media always goes through the relay. A browser cannot join the UDP direct
//! path, so the host's ICE signals are ignored and the host falls back to
//! the relay when its ICE deadline passes.

pub mod reconnect;
#[cfg(target_arch = "wasm32")]
mod wasm;

use protocol::{
    control::Control,
    crypto::{Opener, Sealer},
    frame, kex, pake, Tag,
};
use reconnect::{rejection_error, HOST_ENDED_MESSAGE};

const KEX_MSG: u8 = 0x01;
const KEX_CONFIRM: u8 = 0x02;
/// Both SPAKE2 exchanges must finish this soon after pairing.
pub const KEX_DEADLINE_MS: f64 = 10_000.0;
/// How often the viewer sends a sealed keepalive once authenticated.
pub const KEEPALIVE_INTERVAL_MS: f64 = 15_000.0;
/// The host is gone after this long without a control frame.
pub const DEAD_PEER_TIMEOUT_MS: f64 = 45_000.0;
/// How long to wait for a requested keyframe before asking again.
const KEYFRAME_RETRY_MS: f64 = 500.0;
/// Bytes of video header before the access unit (see `encode::video_header`).
pub const VIDEO_HEADER_LEN: usize = 18;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A binary WebSocket message for the relay.
    Send(Vec<u8>),
    /// Key confirmation passed. The host may still ask its user to accept.
    Authenticated,
    /// Opened video: the 18-byte header, then an H.264 access unit.
    Video(Vec<u8>),
    /// The host's monitors as a JSON array of `MonitorInfo`.
    Monitors(String),
    /// The session is over. The text matches the desktop viewer's errors,
    /// so [`reconnect::after_failure`] can decide what to do next.
    Ended(String),
}

/// The relay `connect` frame that starts a session with `target`.
pub fn connect_frame(target: &str, viewer_id: &str) -> Result<Vec<u8>, String> {
    let nine_digits = |id: &str| id.len() == 9 && id.bytes().all(|byte| byte.is_ascii_digit());
    if !nine_digits(target) {
        return Err("target ID must be 9 digits".into());
    }
    if !nine_digits(viewer_id) {
        return Err("peer ID must be 9 digits".into());
    }
    let connect = Control::Connect { target: target.into(), peer_id: viewer_id.into(), ver: 1 };
    Ok(frame::wrap(Tag::Control, &connect.to_bytes()))
}

/// Viewer-side continuity check of the video sequence, as in host-core: after
/// a gap, frames wait for the next keyframe and the host is asked for one.
#[derive(Debug)]
struct VideoGate {
    next_seq: Option<u64>,
    awaiting_keyframe: bool,
    last_request: Option<f64>,
}

impl VideoGate {
    fn new() -> Self { Self { next_seq: None, awaiting_keyframe: true, last_request: None } }

    fn accept(&mut self, seq: u64, keyframe: bool) -> bool {
        if self.next_seq.is_some_and(|next| seq < next) {
            return false;
        }
        if keyframe {
            self.awaiting_keyframe = false;
        } else if self.awaiting_keyframe || self.next_seq.is_some_and(|next| seq != next) {
            self.awaiting_keyframe = true;
            return false;
        }
        self.next_seq = Some(seq + 1);
        true
    }

    fn should_request(&mut self, now_ms: f64) -> bool {
        if !self.awaiting_keyframe || self.last_request.is_some_and(|at| now_ms - at < KEYFRAME_RETRY_MS) {
            return false;
        }
        self.last_request = Some(now_ms);
        true
    }
}

fn video_seq(plaintext: &[u8]) -> Option<(u64, bool)> {
    let header = plaintext.get(..VIDEO_HEADER_LEN)?;
    Some((u64::from_be_bytes(header[..8].try_into().ok()?), header[13] & 1 == 1))
}

/// Keys and timers of an authenticated session.
struct Live {
    sealer: Sealer,
    opener: Opener,
    control_sealer: Sealer,
    control_opener: Opener,
    gate: VideoGate,
    next_keepalive: f64,
    peer_deadline: f64,
}

impl Live {
    fn control(&mut self, control: &Control) -> Vec<u8> {
        frame::wrap(Tag::Control, &self.control_sealer.seal(&[Tag::Control as u8], &control.to_bytes()))
    }
}

enum Phase {
    Pairing,
    KexMessage(pake::PakeState),
    KexConfirm([u8; 32]),
    Session(Box<Live>),
    Ended,
}

pub struct ViewerSession {
    password: String,
    auth_code: String,
    phase: Phase,
    kex_deadline: f64,
}

impl ViewerSession {
    /// A session that has sent [`connect_frame`] and waits for the relay to
    /// pair it. An empty `auth_code` means none.
    pub fn new(password: &str, auth_code: &str) -> Self {
        Self { password: password.to_owned(), auth_code: auth_code.trim().to_owned(), phase: Phase::Pairing, kex_deadline: f64::INFINITY }
    }

    pub fn is_ended(&self) -> bool { matches!(self.phase, Phase::Ended) }

    pub fn is_authenticated(&self) -> bool { matches!(self.phase, Phase::Session(_)) }

    fn end(&mut self, message: impl Into<String>) -> Vec<Event> {
        self.phase = Phase::Ended;
        vec![Event::Ended(message.into())]
    }

    /// Handle one binary message from the relay.
    pub fn receive(&mut self, message: &[u8], now_ms: f64) -> Vec<Event> {
        match std::mem::replace(&mut self.phase, Phase::Ended) {
            Phase::Ended => Vec::new(),
            Phase::Pairing => self.on_pairing(message, now_ms),
            Phase::KexMessage(state) => {
                match frame::unwrap(message) {
                    Some((Tag::Kex, [KEX_MSG, peer @ ..])) => {
                        let Some(shared) = pake::finish(state, peer) else { return self.end("invalid SPAKE2 message") };
                        let confirm = pake::confirm_tag(&shared, false);
                        self.phase = Phase::KexConfirm(shared);
                        vec![Event::Send(kex_frame(KEX_CONFIRM, &confirm))]
                    }
                    _ => {
                        self.phase = Phase::KexMessage(state);
                        Vec::new()
                    }
                }
            }
            Phase::KexConfirm(shared) => {
                match frame::unwrap(message) {
                    Some((Tag::Kex, [KEX_CONFIRM, tag @ ..])) => {
                        if !pake::verify_peer(&shared, true, tag) { return self.end("key confirmation failed") }
                        self.authenticated(&shared, now_ms)
                    }
                    _ => {
                        self.phase = Phase::KexConfirm(shared);
                        Vec::new()
                    }
                }
            }
            Phase::Session(mut live) => {
                let events = self.on_session(&mut live, message, now_ms);
                // `end` already set Phase::Ended; otherwise the session goes on.
                if !events.iter().any(|event| matches!(event, Event::Ended(_))) { self.phase = Phase::Session(live); }
                events
            }
        }
    }

    fn on_pairing(&mut self, message: &[u8], now_ms: f64) -> Vec<Event> {
        let Some((Tag::Control, body)) = frame::unwrap(message) else { return self.end("invalid pairing message") };
        match Control::from_bytes(body) {
            Some(Control::Paired { .. }) => {
                let (state, my_message) = pake::start(self.password.as_bytes(), false);
                self.phase = Phase::KexMessage(state);
                self.kex_deadline = now_ms + KEX_DEADLINE_MS;
                vec![Event::Send(kex_frame(KEX_MSG, &my_message))]
            }
            Some(Control::Error { code }) => self.end(reconnect::connect_refusal_message(&code).unwrap_or("invalid pairing message")),
            _ => self.end("invalid pairing message"),
        }
    }

    fn authenticated(&mut self, shared: &[u8; 32], now_ms: f64) -> Vec<Event> {
        let (send_key, recv_key) = kex::derive_keys(shared, false);
        let (control_send_key, control_recv_key) = kex::derive_control_keys(shared, false);
        let mut live = Box::new(Live {
            sealer: Sealer::new(send_key),
            opener: Opener::new(recv_key),
            control_sealer: Sealer::new(control_send_key),
            control_opener: Opener::new(control_recv_key),
            gate: VideoGate::new(),
            next_keepalive: now_ms + KEEPALIVE_INTERVAL_MS,
            peer_deadline: now_ms + DEAD_PEER_TIMEOUT_MS,
        });
        // Always sent, empty when there is no code, so a 2FA host answers
        // at once; other hosts ignore it. It must be the first control frame.
        let auth = live.control(&Control::AuthCode { code: self.auth_code.clone() });
        self.phase = Phase::Session(live);
        vec![Event::Send(auth), Event::Authenticated]
    }

    fn on_session(&mut self, live: &mut Live, message: &[u8], now_ms: f64) -> Vec<Event> {
        match frame::unwrap(message) {
            Some((Tag::Control, sealed)) => {
                // Plaintext ICE signals fail to open and are dropped here.
                let Some(plaintext) = live.control_opener.open(&[Tag::Control as u8], sealed) else { return Vec::new() };
                live.peer_deadline = now_ms + DEAD_PEER_TIMEOUT_MS;
                match Control::from_bytes(&plaintext) {
                    Some(Control::Error { code }) => self.end(rejection_error(&code)),
                    Some(Control::Bye { .. }) => self.end(HOST_ENDED_MESSAGE),
                    Some(Control::MonitorList { monitors }) => {
                        vec![Event::Monitors(serde_json::to_string(&monitors).unwrap_or_else(|_| "[]".into()))]
                    }
                    _ => Vec::new(),
                }
            }
            Some((Tag::Video, sealed)) => {
                let Some(plaintext) = live.opener.open(&[Tag::Video as u8], sealed) else { return Vec::new() };
                let Some((seq, keyframe)) = video_seq(&plaintext) else { return Vec::new() };
                let mut events = Vec::new();
                if live.gate.accept(seq, keyframe) {
                    events.push(Event::Video(plaintext));
                }
                if live.gate.should_request(now_ms) {
                    events.push(Event::Send(live.control(&Control::RequestKeyframe)));
                }
                events
            }
            // Audio, file transfer and clipboard are not supported here.
            _ => Vec::new(),
        }
    }

    /// The relay socket closed.
    pub fn closed(&mut self) -> Vec<Event> {
        let message = match self.phase {
            Phase::Ended => return Vec::new(),
            Phase::Pairing => "relay closed while connecting",
            Phase::KexMessage(_) => "connection closed during KEX",
            // A host that rejects our confirm closes at once, and its own
            // confirm can be lost in that close (beta.5 aborts its writer).
            // So a close here almost always means a wrong password. The
            // desktop viewer reads it as a drop and retries for 30 s.
            Phase::KexConfirm(_) => "key confirmation failed",
            Phase::Session(_) => "connection closed during session",
        };
        self.end(message)
    }

    /// Run timers: keepalives and the KEX and dead-peer deadlines. Call it
    /// every second or so.
    pub fn tick(&mut self, now_ms: f64) -> Vec<Event> {
        match &mut self.phase {
            Phase::KexMessage(_) if now_ms >= self.kex_deadline => self.end("connection closed during KEX"),
            Phase::KexConfirm(_) if now_ms >= self.kex_deadline => self.end("connection closed during KEX confirmation"),
            Phase::Session(live) => {
                if now_ms >= live.peer_deadline {
                    return self.end(format!(
                        "connection closed: host stopped responding for {}s",
                        (DEAD_PEER_TIMEOUT_MS / 1000.0) as u64
                    ));
                }
                if now_ms >= live.next_keepalive {
                    live.next_keepalive = now_ms + KEEPALIVE_INTERVAL_MS;
                    return vec![Event::Send(live.control(&Control::KeepAlive))];
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn live(&mut self) -> Option<&mut Live> {
        match &mut self.phase {
            Phase::Session(live) => Some(live),
            _ => None,
        }
    }

    /// Seal an encoded `protocol::input::InputEvent`. `None` before the
    /// session is authenticated.
    pub fn input(&mut self, event: &[u8]) -> Option<Vec<u8>> {
        let live = self.live()?;
        Some(frame::wrap(Tag::Input, &live.sealer.seal(&[Tag::Input as u8], event)))
    }

    /// Ask the host for a keyframe, e.g. after a decoder error.
    pub fn request_keyframe(&mut self) -> Option<Vec<u8>> {
        Some(self.live()?.control(&Control::RequestKeyframe))
    }

    pub fn select_monitor(&mut self, id: u32) -> Option<Vec<u8>> {
        Some(self.live()?.control(&Control::SelectMonitor { id }))
    }

    /// A sealed goodbye so the host ends its session at once. Ends this one.
    pub fn bye(&mut self) -> Option<Vec<u8>> {
        let bye = self.live()?.control(&Control::Bye { session: String::new() });
        self.phase = Phase::Ended;
        Some(bye)
    }
}

fn kex_frame(sub: u8, payload: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(1 + payload.len());
    body.push(sub);
    body.extend_from_slice(payload);
    frame::wrap(Tag::Kex, &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paired() -> Vec<u8> {
        frame::wrap(Tag::Control, &Control::Paired { session: "s1".into(), peer_role: "viewer".into(), peer_id: "123456789".into() }.to_bytes())
    }

    fn sent(events: &[Event]) -> Vec<Vec<u8>> {
        events.iter().filter_map(|event| match event { Event::Send(bytes) => Some(bytes.clone()), _ => None }).collect()
    }

    /// A host-side mirror of roles.rs, enough to authenticate a viewer.
    struct FakeHost { shared: [u8; 32], control_sealer: Sealer, control_opener: Opener, sealer: Sealer, opener: Opener }

    fn authenticate(viewer: &mut ViewerSession, host_password: &str) -> Result<FakeHost, Vec<Event>> {
        let out = viewer.receive(&paired(), 0.0);
        let viewer_kex = sent(&out).remove(0);
        let (state, host_msg) = pake::start(host_password.as_bytes(), true);
        let (Tag::Kex, [KEX_MSG, viewer_msg @ ..]) = frame::unwrap(&viewer_kex).unwrap() else { panic!("expected a KEX message") };
        let shared = pake::finish(state, viewer_msg).unwrap();
        let out = viewer.receive(&kex_frame(KEX_MSG, &host_msg), 1.0);
        let viewer_confirm = sent(&out).remove(0);
        let (Tag::Kex, [KEX_CONFIRM, tag @ ..]) = frame::unwrap(&viewer_confirm).unwrap() else { panic!("expected a confirm") };
        assert_eq!(pake::verify_peer(&shared, false, tag), host_password == viewer.password);
        let out = viewer.receive(&kex_frame(KEX_CONFIRM, &pake::confirm_tag(&shared, true)), 2.0);
        if viewer.is_ended() { return Err(out); }
        let (send, recv) = kex::derive_keys(&shared, true);
        let (control_send, control_recv) = kex::derive_control_keys(&shared, true);
        let mut host = FakeHost { shared, control_sealer: Sealer::new(control_send), control_opener: Opener::new(control_recv), sealer: Sealer::new(send), opener: Opener::new(recv) };
        let auth = sent(&out).remove(0);
        let (Tag::Control, sealed) = frame::unwrap(&auth).unwrap() else { panic!() };
        let plain = host.control_opener.open(&[Tag::Control as u8], sealed).unwrap();
        assert!(matches!(Control::from_bytes(&plain), Some(Control::AuthCode { .. })));
        assert!(out.contains(&Event::Authenticated));
        Ok(host)
    }

    impl FakeHost {
        fn control(&mut self, control: &Control) -> Vec<u8> {
            frame::wrap(Tag::Control, &self.control_sealer.seal(&[Tag::Control as u8], &control.to_bytes()))
        }
        fn video(&mut self, seq: u64, keyframe: bool) -> Vec<u8> {
            let mut plain = seq.to_be_bytes().to_vec();
            plain.extend_from_slice(&[0, 0, 0, 0, 1, u8::from(keyframe), 0, 64, 0, 64, 0, 0, 0, 1, 0x65]);
            frame::wrap(Tag::Video, &self.sealer.seal(&[Tag::Video as u8], &plain))
        }
        fn open_control(&mut self, message: &[u8]) -> Option<Control> {
            let (Tag::Control, sealed) = frame::unwrap(message)? else { return None };
            Control::from_bytes(&self.control_opener.open(&[Tag::Control as u8], sealed)?)
        }
    }

    #[test]
    fn connect_frame_validates_ids() {
        assert!(connect_frame("12345678", "123456789").is_err());
        let bytes = connect_frame("123456789", "987654321").unwrap();
        let (Tag::Control, body) = frame::unwrap(&bytes).unwrap() else { panic!() };
        assert_eq!(Control::from_bytes(body), Some(Control::Connect { target: "123456789".into(), peer_id: "987654321".into(), ver: 1 }));
    }

    #[test]
    fn unavailable_peer_uses_the_desktop_message() {
        use protocol::control::ERROR_PEER_UNAVAILABLE;
        let mut viewer = ViewerSession::new("pw", "");
        let error = frame::wrap(Tag::Control, &Control::Error { code: ERROR_PEER_UNAVAILABLE.into() }.to_bytes());
        assert_eq!(viewer.receive(&error, 0.0), vec![Event::Ended(reconnect::PEER_UNAVAILABLE_MESSAGE.into())]);
    }

    #[test]
    fn relay_refusals_use_the_desktop_messages() {
        let refused = |code: &str| {
            let mut viewer = ViewerSession::new("pw", "");
            viewer.receive(&frame::wrap(Tag::Control, &Control::Error { code: code.into() }.to_bytes()), 0.0)
        };
        assert_eq!(refused(protocol::control::ERROR_CONNECT_RATE), vec![Event::Ended(reconnect::RELAY_LIMITED_MESSAGE.into())]);
        assert_eq!(refused("from_a_newer_relay"), vec![Event::Ended("invalid pairing message".into())]);
    }

    #[test]
    fn wrong_password_fails_key_confirmation() {
        let mut viewer = ViewerSession::new("right", "");
        let Err(out) = authenticate(&mut viewer, "wrong") else { panic!("must not authenticate") };
        assert_eq!(out, vec![Event::Ended("key confirmation failed".into())]);
    }

    #[test]
    fn session_opens_video_seals_input_and_handles_control() {
        let mut viewer = ViewerSession::new("pw", "123456");
        let mut host = authenticate(&mut viewer, "pw").unwrap();
        assert_ne!(host.shared, [0; 32]);
        // Plaintext ICE from the host is ignored.
        let ice = frame::wrap(Tag::Control, &Control::Ice { session: "s1".into(), cand: "cred:a:b".into() }.to_bytes());
        assert!(viewer.receive(&ice, 3.0).is_empty());
        let monitors = host.control(&Control::MonitorList { monitors: vec![] });
        assert_eq!(viewer.receive(&monitors, 4.0), vec![Event::Monitors("[]".into())]);
        let out = viewer.receive(&host.video(0, true), 5.0);
        assert!(matches!(&out[..], [Event::Video(frame)] if frame.len() > VIDEO_HEADER_LEN));
        // A gap holds frames back and asks for a keyframe once per 500 ms.
        let out = viewer.receive(&host.video(2, false), 6.0);
        assert_eq!(out.len(), 1);
        assert_eq!(host.open_control(&sent(&out)[0]), Some(Control::RequestKeyframe));
        assert!(viewer.receive(&host.video(3, false), 7.0).is_empty());
        assert!(matches!(&viewer.receive(&host.video(4, true), 8.0)[..], [Event::Video(_)]));

        let input = protocol::input::InputEvent::MouseMove { x: 1, y: 2 }.to_bytes(9);
        let sealed = viewer.input(&input).unwrap();
        let (Tag::Input, body) = frame::unwrap(&sealed).unwrap() else { panic!() };
        assert_eq!(host.opener.open(&[Tag::Input as u8], body), Some(input));

        let bye = host.control(&Control::Bye { session: String::new() });
        assert_eq!(viewer.receive(&bye, 9.0), vec![Event::Ended(HOST_ENDED_MESSAGE.into())]);
        assert!(viewer.input(&[1]).is_none());
    }

    #[test]
    fn host_rejection_carries_the_code() {
        let mut viewer = ViewerSession::new("pw", "");
        let mut host = authenticate(&mut viewer, "pw").unwrap();
        let reject = host.control(&Control::Error { code: "2fa_required".into() });
        assert_eq!(viewer.receive(&reject, 3.0), vec![Event::Ended("session rejected by host: 2fa_required".into())]);
    }

    #[test]
    fn timers_send_keepalives_and_notice_a_silent_host() {
        let mut viewer = ViewerSession::new("pw", "");
        let mut host = authenticate(&mut viewer, "pw").unwrap();
        assert!(viewer.tick(1_000.0).is_empty());
        let out = viewer.tick(2.0 + KEEPALIVE_INTERVAL_MS);
        assert_eq!(host.open_control(&sent(&out)[0]), Some(Control::KeepAlive));
        let out = viewer.tick(2.0 + DEAD_PEER_TIMEOUT_MS);
        assert_eq!(out, vec![Event::Ended("connection closed: host stopped responding for 45s".into())]);
    }

    #[test]
    fn kex_has_a_deadline_and_closes_name_the_phase() {
        let mut viewer = ViewerSession::new("pw", "");
        viewer.receive(&paired(), 0.0);
        assert!(viewer.tick(KEX_DEADLINE_MS - 1.0).is_empty());
        assert_eq!(viewer.tick(KEX_DEADLINE_MS), vec![Event::Ended("connection closed during KEX".into())]);
        let mut viewer = ViewerSession::new("pw", "");
        assert_eq!(viewer.closed(), vec![Event::Ended("relay closed while connecting".into())]);
        assert!(viewer.closed().is_empty());
    }

    #[test]
    fn bye_is_sealed_and_ends_the_session() {
        let mut viewer = ViewerSession::new("pw", "");
        assert!(viewer.bye().is_none());
        let mut host = authenticate(&mut viewer, "pw").unwrap();
        let bye = viewer.bye().unwrap();
        assert_eq!(host.open_control(&bye), Some(Control::Bye { session: String::new() }));
        assert!(viewer.is_ended());
    }
}
