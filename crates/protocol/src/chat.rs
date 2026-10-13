pub const MAX_CHAT: usize = 8192;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMsg {
    pub text: String,
}

impl ChatMsg {
    pub fn to_bytes(&self) -> Vec<u8> {
        let text = self.text.as_bytes();
        let length = u32::try_from(text.len()).expect("chat text exceeds u32 length");
        let mut bytes = Vec::with_capacity(4 + text.len());
        bytes.extend_from_slice(&length.to_be_bytes());
        bytes.extend_from_slice(text);
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let length = usize::try_from(u32::from_be_bytes(bytes.get(..4)?.try_into().ok()?)).ok()?;
        if length > MAX_CHAT {
            return None;
        }
        let text_start: usize = 4;
        let text_end = text_start.checked_add(length)?;
        if text_end != bytes.len() {
            return None;
        }
        Some(Self {
            text: String::from_utf8(bytes.get(text_start..text_end)?.to_vec()).ok()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_empty_unicode_and_regular_text() {
        for text in ["", "hello", "こんにちは 🌍"] {
            let message = ChatMsg { text: text.into() };
            assert_eq!(ChatMsg::from_bytes(&message.to_bytes()), Some(message));
        }
    }

    #[test]
    fn rejects_truncated_and_garbage_bytes() {
        let message = ChatMsg {
            text: "chat".into(),
        };
        let bytes = message.to_bytes();
        assert!(ChatMsg::from_bytes(&bytes[..bytes.len() - 1]).is_none());
        assert!(ChatMsg::from_bytes(&[0xff, 0x00]).is_none());
    }

    #[test]
    fn rejects_oversize_declared_length() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&u32::try_from(MAX_CHAT + 1).unwrap().to_be_bytes());
        assert!(ChatMsg::from_bytes(&bytes).is_none());
    }
}
