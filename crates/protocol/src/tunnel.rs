//! TCP tunnel (port forwarding) messages on the Data channel.
//!
//! The viewer listens on a local port; each connection to it becomes a tunnel
//! connection `id` that the host opens to a target on its own network. Bytes
//! then flow both ways in `Data` messages. A side keeps at most
//! [`TUNNEL_WINDOW`] bytes of a connection unacknowledged, and acknowledges
//! the peer's bytes once they are written to its socket, so one slow socket
//! can flood neither the shared Data channel nor the other side's memory.

use crate::wire::{put_bytes, put_u16, put_u64, Reader};

/// Most payload bytes in one `Data` message.
pub const MAX_TUNNEL_CHUNK: usize = 32 * 1024;
/// Bytes of one connection a side may have sent without the peer's `Ack`.
pub const TUNNEL_WINDOW: u64 = 512 * 1024;
/// Longest target host (a DNS name or an IP address) in an `Open`.
pub const MAX_TUNNEL_HOST: usize = 253;
/// Longest code in a `Refused`.
const MAX_TUNNEL_CODE: usize = 64;

/// `Refused` codes; viewers show a generic message for codes they don't know.
/// The host has tunnels turned off for this viewer.
pub const TUNNEL_NOT_ALLOWED: &str = "not_allowed";
/// The host could not connect to the target.
pub const TUNNEL_UNREACHABLE: &str = "unreachable";
/// The session already has as many tunnel connections as the host allows.
pub const TUNNEL_LIMIT: &str = "limit";
/// The target host or port is not valid.
pub const TUNNEL_INVALID: &str = "invalid";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TunnelMsg {
    /// Host → viewer, once per session: this host takes tunnel connections
    /// (while its tunnel permission is on). Viewers only offer tunnels after
    /// it; older hosts never send it.
    Available,
    /// Viewer → host: connect to `host:port` for connection `id`.
    Open { id: u64, host: String, port: u16 },
    /// Host → viewer: connection `id` is up.
    Opened { id: u64 },
    /// Host → viewer: connection `id` was not opened; `code` is one of the
    /// `TUNNEL_*` codes, or a newer one.
    Refused { id: u64, code: String },
    /// Either way: the next bytes of connection `id`.
    Data { id: u64, data: Vec<u8> },
    /// Either way: the sender has written `written` bytes of the peer's data
    /// on connection `id` to its socket so far.
    Ack { id: u64, written: u64 },
    /// Either way: the sender's socket reached its end; no more `Data`
    /// follows on `id`.
    Eof { id: u64 },
    /// Either way: connection `id` is finished or failed; forget it.
    Close { id: u64 },
}

impl TunnelMsg {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        match self {
            Self::Available => bytes.push(1),
            Self::Open { id, host, port } => {
                bytes.push(2);
                put_u64(&mut bytes, *id);
                put_bytes(&mut bytes, host.as_bytes());
                put_u16(&mut bytes, *port);
            }
            Self::Opened { id } => {
                bytes.push(3);
                put_u64(&mut bytes, *id);
            }
            Self::Refused { id, code } => {
                bytes.push(4);
                put_u64(&mut bytes, *id);
                put_bytes(&mut bytes, code.as_bytes());
            }
            Self::Data { id, data } => {
                bytes.push(5);
                put_u64(&mut bytes, *id);
                put_bytes(&mut bytes, data);
            }
            Self::Ack { id, written } => {
                bytes.push(6);
                put_u64(&mut bytes, *id);
                put_u64(&mut bytes, *written);
            }
            Self::Eof { id } => {
                bytes.push(7);
                put_u64(&mut bytes, *id);
            }
            Self::Close { id } => {
                bytes.push(8);
                put_u64(&mut bytes, *id);
            }
        }
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<TunnelMsg> {
        let mut reader = Reader::new(bytes);
        let message = match reader.take_u8()? {
            1 => Self::Available,
            2 => {
                let id = reader.take_u64()?;
                let host = reader.take_string()?;
                if host.len() > MAX_TUNNEL_HOST {
                    return None;
                }
                Self::Open { id, host, port: reader.take_u16()? }
            }
            3 => Self::Opened { id: reader.take_u64()? },
            4 => {
                let id = reader.take_u64()?;
                let code = reader.take_string()?;
                if code.len() > MAX_TUNNEL_CODE {
                    return None;
                }
                Self::Refused { id, code }
            }
            5 => {
                let id = reader.take_u64()?;
                let data = reader.take_bytes()?;
                if data.len() > MAX_TUNNEL_CHUNK {
                    return None;
                }
                Self::Data { id, data }
            }
            6 => Self::Ack { id: reader.take_u64()?, written: reader.take_u64()? },
            7 => Self::Eof { id: reader.take_u64()? },
            8 => Self::Close { id: reader.take_u64()? },
            _ => return None,
        };
        reader.is_empty().then_some(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_message_roundtrips() {
        for message in [
            TunnelMsg::Available,
            TunnelMsg::Open { id: 1, host: "192.168.1.20".into(), port: 3389 },
            TunnelMsg::Open { id: u64::MAX, host: "printer.local".into(), port: 1 },
            TunnelMsg::Opened { id: 2 },
            TunnelMsg::Refused { id: 3, code: TUNNEL_UNREACHABLE.into() },
            TunnelMsg::Data { id: 4, data: vec![0, 255, 7] },
            TunnelMsg::Data { id: 4, data: vec![9; MAX_TUNNEL_CHUNK] },
            TunnelMsg::Ack { id: 5, written: TUNNEL_WINDOW * 3 },
            TunnelMsg::Eof { id: 6 },
            TunnelMsg::Close { id: 7 },
        ] {
            assert_eq!(TunnelMsg::from_bytes(&message.to_bytes()), Some(message));
        }
    }

    #[test]
    fn rejects_oversize_chunks_hosts_and_codes() {
        let chunk = TunnelMsg::Data { id: 1, data: vec![0; MAX_TUNNEL_CHUNK + 1] };
        assert!(TunnelMsg::from_bytes(&chunk.to_bytes()).is_none());
        let open = TunnelMsg::Open { id: 1, host: "a".repeat(MAX_TUNNEL_HOST + 1), port: 22 };
        assert!(TunnelMsg::from_bytes(&open.to_bytes()).is_none());
        let refused = TunnelMsg::Refused { id: 1, code: "x".repeat(MAX_TUNNEL_CODE + 1) };
        assert!(TunnelMsg::from_bytes(&refused.to_bytes()).is_none());
    }

    #[test]
    fn rejects_empty_unknown_truncated_and_trailing_bytes() {
        assert!(TunnelMsg::from_bytes(&[]).is_none());
        assert!(TunnelMsg::from_bytes(&[0]).is_none());
        assert!(TunnelMsg::from_bytes(&[9]).is_none());
        let bytes = TunnelMsg::Ack { id: 1, written: 2 }.to_bytes();
        assert!(TunnelMsg::from_bytes(&bytes[..bytes.len() - 1]).is_none());
        let mut trailing = TunnelMsg::Close { id: 1 }.to_bytes();
        trailing.push(0);
        assert!(TunnelMsg::from_bytes(&trailing).is_none());
    }
}
