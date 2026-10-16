#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Tag {
    Control = 0x01,
    Kex = 0x02,
    Video = 0x10,
    Audio = 0x11,
    Input = 0x12,
    Data = 0x13,
    /// An extra per-monitor video stream (multi-monitor windows). The body is
    /// a 4-byte big-endian monitor id, then the same payload as `Video`. Peers
    /// that predate it hit the catch-all and ignore it, so they keep the
    /// single-stream view.
    VideoAux = 0x14,
}

impl Tag {
    pub fn from_u8(b: u8) -> Option<Tag> {
        Some(match b {
            0x01 => Tag::Control,
            0x02 => Tag::Kex,
            0x10 => Tag::Video,
            0x11 => Tag::Audio,
            0x12 => Tag::Input,
            0x13 => Tag::Data,
            0x14 => Tag::VideoAux,
            _ => return None,
        })
    }
}

pub fn wrap(tag: Tag, body: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(1 + body.len());
    v.push(tag as u8);
    v.extend_from_slice(body);
    v
}

pub fn unwrap(msg: &[u8]) -> Option<(Tag, &[u8])> {
    let (&first, rest) = msg.split_first()?;
    Some((Tag::from_u8(first)?, rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_then_unwrap_roundtrips() {
        let body = b"hello";
        let msg = wrap(Tag::Video, body);
        let (tag, out) = unwrap(&msg).expect("valid frame");
        assert_eq!(tag, Tag::Video);
        assert_eq!(out, body);
    }

    #[test]
    fn unwrap_rejects_empty() {
        assert!(unwrap(&[]).is_none());
    }

    #[test]
    fn unwrap_rejects_unknown_tag() {
        assert!(unwrap(&[0xFF, 1, 2]).is_none());
    }

    #[test]
    fn video_aux_tag_roundtrips() {
        let msg = wrap(Tag::VideoAux, b"body");
        assert_eq!(msg[0], 0x14);
        assert_eq!(unwrap(&msg), Some((Tag::VideoAux, &b"body"[..])));
    }
}
