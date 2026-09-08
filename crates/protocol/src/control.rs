use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Control {
    Register { peer_id: String, ver: u32 },
    Connect { target: String, ver: u32 },
    Paired { session: String, peer_role: String },
    Error { code: String },
    Bye { session: String },
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
        let c = Control::Register { peer_id: "123-456-789".into(), ver: 1 };
        let bytes = c.to_bytes();
        assert_eq!(Control::from_bytes(&bytes), Some(c));
    }

    #[test]
    fn wire_uses_t_discriminator() {
        let c = Control::Connect { target: "1".into(), ver: 1 };
        let json = String::from_utf8(c.to_bytes()).unwrap();
        assert!(json.contains("\"t\":\"connect\""));
    }
}
