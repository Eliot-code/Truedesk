use serde::{Deserialize, Serialize};

/// `Control::Error` code the relay sends when a `register` names an ID that is
/// leased to a different token.
pub const ERROR_ID_TAKEN: &str = "id_taken";
/// `Control::Error` code the relay sends when a `connect` names an ID that is
/// not waiting for a viewer: offline, or already in a session.
pub const ERROR_PEER_UNAVAILABLE: &str = "peer_unavailable";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Control {
    Register { peer_id: String, token: String, ver: u32 },
    Connect { target: String, peer_id: String, ver: u32 },
    Paired { session: String, peer_role: String, peer_id: String },
    Ice { session: String, cand: String },
    Error { code: String },
    Bye { session: String },
    MonitorList { monitors: Vec<MonitorInfo> },
    SelectMonitor { id: u32 },
    AuthCode { code: String },
    /// Sealed liveness probe both peers send periodically on the relay socket
    /// after authentication. It keeps idle relay sockets open (a direct
    /// session carries no relay traffic otherwise) and lets each side notice
    /// a vanished peer when keepalives stop arriving.
    KeepAlive,
    /// Viewer → host: the decoder lost its reference (a gap or decode error),
    /// so the host should send a keyframe as soon as it can.
    RequestKeyframe,
    /// Viewer → host on the relay socket, to measure the relay round trip.
    /// `sent_us` is on the viewer's clock; the host echoes it in a `Pong`.
    /// beta.5 and older peers cannot parse it and ignore it, so the viewer
    /// just never gets an answer from them.
    Ping { sent_us: u64 },
    /// Host → viewer: the answer to a `Ping`, echoing its `sent_us`.
    Pong { sent_us: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorInfo {
    pub id: u32,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub is_primary: bool,
}

impl Control {
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("control serializes")
    }
    pub fn from_bytes(b: &[u8]) -> Option<Control> {
        serde_json::from_slice(b).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_roundtrips() {
        let c = Control::Register { peer_id: "123-456-789".into(), token: "00".repeat(32), ver: 1 };
        let bytes = c.to_bytes();
        assert_eq!(Control::from_bytes(&bytes), Some(c));
    }

    #[test]
    fn wire_uses_t_discriminator() {
        let c = Control::Connect { target: "1".into(), peer_id: "2".into(), ver: 1 };
        let json = String::from_utf8(c.to_bytes()).unwrap();
        assert!(json.contains("\"t\":\"connect\""));
    }

    #[test]
    fn paired_with_peer_id_roundtrips() {
        let c = Control::Paired { session: "s1".into(), peer_role: "host".into(), peer_id: "987654321".into() };
        assert_eq!(Control::from_bytes(&c.to_bytes()), Some(c));
    }

    #[test]
    fn ice_roundtrips() {
        let c = Control::Ice {
            session: "s1".into(),
            cand: "4257992714 1 udp 2130706431 10.2.0.2 57012 typ host".into(),
        };
        let bytes = c.to_bytes();
        assert_eq!(Control::from_bytes(&bytes), Some(c));
    }

    #[test]
    fn ice_uses_t_discriminator() {
        let c = Control::Ice {
            session: "s1".into(),
            cand: "candidate".into(),
        };
        let json = String::from_utf8(c.to_bytes()).unwrap();
        assert!(json.contains("\"t\":\"ice\""));
    }

    #[test]
    fn monitor_list_roundtrips() {
        let c = Control::MonitorList {
            monitors: vec![MonitorInfo {
                id: 42,
                name: "Built-in Display".into(),
                width: 2560,
                height: 1600,
                is_primary: true,
            }],
        };
        assert_eq!(Control::from_bytes(&c.to_bytes()), Some(c));
    }

    #[test]
    fn select_monitor_roundtrips() {
        let c = Control::SelectMonitor { id: 7 };
        assert_eq!(Control::from_bytes(&c.to_bytes()), Some(c));
    }

    #[test]
    fn auth_code_roundtrips() {
        let c = Control::AuthCode { code: "123456".into() };
        assert_eq!(Control::from_bytes(&c.to_bytes()), Some(c));
    }

    #[test]
    fn request_keyframe_roundtrips() {
        let bytes = Control::RequestKeyframe.to_bytes();
        assert_eq!(String::from_utf8(bytes.clone()).unwrap(), r#"{"t":"request_keyframe"}"#);
        assert_eq!(Control::from_bytes(&bytes), Some(Control::RequestKeyframe));
    }

    #[test]
    fn keepalive_roundtrips_and_uses_t_discriminator() {
        let bytes = Control::KeepAlive.to_bytes();
        assert_eq!(String::from_utf8(bytes.clone()).unwrap(), r#"{"t":"keep_alive"}"#);
        assert_eq!(Control::from_bytes(&bytes), Some(Control::KeepAlive));
    }

    #[test]
    fn ping_and_pong_roundtrip() {
        let ping = Control::Ping { sent_us: 1_234_567 };
        assert_eq!(String::from_utf8(ping.to_bytes()).unwrap(), r#"{"t":"ping","sent_us":1234567}"#);
        assert_eq!(Control::from_bytes(&ping.to_bytes()), Some(ping));
        let pong = Control::Pong { sent_us: 7 };
        assert_eq!(String::from_utf8(pong.to_bytes()).unwrap(), r#"{"t":"pong","sent_us":7}"#);
        assert_eq!(Control::from_bytes(&pong.to_bytes()), Some(pong));
    }

    /// The control enum as v0.1.0-beta.5 shipped it. Old peers parse sealed
    /// control with this, and anything that fails to parse falls through to
    /// a `_` arm in their session loops.
    mod beta5 {
        use super::MonitorInfo;
        use serde::Deserialize;

        #[derive(Debug, Deserialize)]
        #[serde(tag = "t", rename_all = "snake_case")]
        #[allow(dead_code)]
        pub enum Control {
            Register { peer_id: String, token: String, ver: u32 },
            Connect { target: String, peer_id: String, ver: u32 },
            Paired { session: String, peer_role: String, peer_id: String },
            Ice { session: String, cand: String },
            Error { code: String },
            Bye { session: String },
            MonitorList { monitors: Vec<MonitorInfo> },
            SelectMonitor { id: u32 },
            AuthCode { code: String },
            KeepAlive,
            RequestKeyframe,
        }

        pub fn from_bytes(b: &[u8]) -> Option<Control> { serde_json::from_slice(b).ok() }
    }

    #[test]
    fn beta5_peers_skip_ping_and_pong_but_read_everything_else() {
        assert!(beta5::from_bytes(&Control::Ping { sent_us: 1 }.to_bytes()).is_none());
        assert!(beta5::from_bytes(&Control::Pong { sent_us: 1 }.to_bytes()).is_none());
        for known in [Control::KeepAlive, Control::RequestKeyframe, Control::Bye { session: String::new() }, Control::SelectMonitor { id: 2 }] {
            assert!(beta5::from_bytes(&known.to_bytes()).is_some(), "{known:?}");
        }
    }

    #[test]
    fn unknown_kinds_from_newer_peers_are_ignored() {
        assert_eq!(Control::from_bytes(br#"{"t":"something_newer","x":1}"#), None);
    }

    #[test]
    fn bye_roundtrips_and_uses_t_discriminator() {
        let c = Control::Bye { session: "s1".into() };
        let bytes = c.to_bytes();
        assert!(String::from_utf8(bytes.clone()).unwrap().contains("\"t\":\"bye\""));
        assert_eq!(Control::from_bytes(&bytes), Some(c));
    }
}
