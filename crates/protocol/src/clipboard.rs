pub const MAX_CLIPBOARD: usize = 1_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardMsg {
    pub seq: u64,
    pub text: String,
}

impl ClipboardMsg {
    pub fn to_bytes(&self) -> Vec<u8> {
        let text = self.text.as_bytes();
        let length = u32::try_from(text.len()).expect("clipboard text exceeds u32 length");
        let mut bytes = Vec::with_capacity(8 + 4 + text.len());
        bytes.extend_from_slice(&self.seq.to_be_bytes());
        bytes.extend_from_slice(&length.to_be_bytes());
        bytes.extend_from_slice(text);
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let seq = u64::from_be_bytes(bytes.get(..8)?.try_into().ok()?);
        let length_start: usize = 8;
        let length_end = length_start.checked_add(4)?;
        let length = usize::try_from(u32::from_be_bytes(
            bytes.get(length_start..length_end)?.try_into().ok()?,
        ))
        .ok()?;
        if length > MAX_CLIPBOARD {
            return None;
        }
        let text_start = length_end;
        let text_end = text_start.checked_add(length)?;
        if text_end != bytes.len() {
            return None;
        }
        Some(Self {
            seq,
            text: String::from_utf8(bytes.get(text_start..text_end)?.to_vec()).ok()?,
        })
    }
}

/// Largest clipboard image we send or accept, in encoded bytes. Kept under
/// the reliable Data channel's in-flight budget so one image can't wedge it.
pub const MAX_CLIPBOARD_IMAGE: usize = 3_000_000;

/// A clipboard image as opaque encoded bytes (PNG). The host encodes and
/// decodes; the protocol only carries them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardImageMsg {
    pub seq: u64,
    pub image: Vec<u8>,
}

impl ClipboardImageMsg {
    pub fn to_bytes(&self) -> Vec<u8> {
        let length = u32::try_from(self.image.len()).expect("clipboard image exceeds u32 length");
        let mut bytes = Vec::with_capacity(8 + 4 + self.image.len());
        bytes.extend_from_slice(&self.seq.to_be_bytes());
        bytes.extend_from_slice(&length.to_be_bytes());
        bytes.extend_from_slice(&self.image);
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let seq = u64::from_be_bytes(bytes.get(..8)?.try_into().ok()?);
        let length = usize::try_from(u32::from_be_bytes(bytes.get(8..12)?.try_into().ok()?)).ok()?;
        if length > MAX_CLIPBOARD_IMAGE {
            return None;
        }
        let end = 12usize.checked_add(length)?;
        if end != bytes.len() {
            return None;
        }
        Some(Self { seq, image: bytes.get(12..end)?.to_vec() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_empty_unicode_and_regular_text() {
        for text in ["", "hello", "こんにちは 🌍"] {
            let message = ClipboardMsg {
                seq: 42,
                text: text.into(),
            };
            assert_eq!(ClipboardMsg::from_bytes(&message.to_bytes()), Some(message));
        }
    }

    #[test]
    fn rejects_truncated_and_garbage_bytes() {
        let message = ClipboardMsg {
            seq: 7,
            text: "clipboard".into(),
        };
        let bytes = message.to_bytes();
        assert!(ClipboardMsg::from_bytes(&bytes[..bytes.len() - 1]).is_none());
        assert!(ClipboardMsg::from_bytes(&[0xff, 0x00]).is_none());
    }

    #[test]
    fn rejects_oversize_declared_length() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0u64.to_be_bytes());
        bytes.extend_from_slice(&u32::try_from(MAX_CLIPBOARD + 1).unwrap().to_be_bytes());
        assert!(ClipboardMsg::from_bytes(&bytes).is_none());
    }

    #[test]
    fn image_messages_round_trip_and_reject_bad_bytes() {
        let message = ClipboardImageMsg { seq: 9, image: vec![0u8, 1, 2, 3, 255] };
        assert_eq!(ClipboardImageMsg::from_bytes(&message.to_bytes()), Some(message.clone()));
        let bytes = message.to_bytes();
        assert!(ClipboardImageMsg::from_bytes(&bytes[..bytes.len() - 1]).is_none());
        let mut oversize = Vec::new();
        oversize.extend_from_slice(&0u64.to_be_bytes());
        oversize.extend_from_slice(&u32::try_from(MAX_CLIPBOARD_IMAGE + 1).unwrap().to_be_bytes());
        assert!(ClipboardImageMsg::from_bytes(&oversize).is_none());
    }
}
