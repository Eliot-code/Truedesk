const MAGIC: &[u8; 4] = b"OPUS";
const HEADER_LEN: usize = 15;

/// One Opus packet. The parameters remain on the wire so receivers can
/// validate and decode packets without session-global audio state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioFrame {
    pub sample_rate: u32,
    pub channels: u8,
    pub frame_duration_ms: u16,
    pub payload: Vec<u8>,
}

impl AudioFrame {
    pub fn to_bytes(&self) -> Vec<u8> {
        let count = u32::try_from(self.payload.len()).expect("audio packet is too large");
        let mut bytes = Vec::with_capacity(HEADER_LEN + self.payload.len());
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&self.sample_rate.to_be_bytes());
        bytes.push(self.channels);
        bytes.extend_from_slice(&self.frame_duration_ms.to_be_bytes());
        bytes.extend_from_slice(&count.to_be_bytes());
        bytes.extend_from_slice(&self.payload);
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < HEADER_LEN || &bytes[..4] != MAGIC { return None; }
        let sample_rate = u32::from_be_bytes(bytes[4..8].try_into().ok()?);
        let channels = bytes[8];
        let frame_duration_ms = u16::from_be_bytes(bytes[9..11].try_into().ok()?);
        let count = usize::try_from(u32::from_be_bytes(bytes[11..15].try_into().ok()?)).ok()?;
        if sample_rate != 48_000 || !(channels == 1 || channels == 2) || frame_duration_ms != 20 { return None; }
        if bytes.len() != HEADER_LEN.checked_add(count)? || count == 0 { return None; }
        Some(Self { sample_rate, channels, frame_duration_ms, payload: bytes[HEADER_LEN..].to_vec() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opus_audio_frame_roundtrips_and_rejects_bad_lengths() {
        let frame = AudioFrame { sample_rate: 48_000, channels: 2, frame_duration_ms: 20, payload: vec![0, 1, 2, 255] };
        assert_eq!(AudioFrame::from_bytes(&frame.to_bytes()), Some(frame));
        assert!(AudioFrame::from_bytes(&[0; HEADER_LEN]).is_none());
        let mut truncated = AudioFrame { sample_rate: 48_000, channels: 1, frame_duration_ms: 20, payload: vec![1, 2] }.to_bytes();
        truncated.pop();
        assert!(AudioFrame::from_bytes(&truncated).is_none());
    }
}
