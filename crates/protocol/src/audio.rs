#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioFrame {
    pub sample_rate: u32,
    pub channels: u8,
    pub samples: Vec<i16>,
}

impl AudioFrame {
    pub fn to_bytes(&self) -> Vec<u8> {
        let count = u32::try_from(self.samples.len()).expect("audio frame is too large");
        let mut bytes = Vec::with_capacity(9 + self.samples.len() * 2);
        bytes.extend_from_slice(&self.sample_rate.to_be_bytes());
        bytes.push(self.channels);
        bytes.extend_from_slice(&count.to_be_bytes());
        for sample in &self.samples { bytes.extend_from_slice(&sample.to_be_bytes()); }
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 9 { return None; }
        let sample_rate = u32::from_be_bytes(bytes[0..4].try_into().ok()?);
        let channels = bytes[4];
        if channels == 0 { return None; }
        let count = usize::try_from(u32::from_be_bytes(bytes[5..9].try_into().ok()?)).ok()?;
        let payload_len = count.checked_mul(2)?;
        if bytes.len() != 9usize.checked_add(payload_len)? { return None; }
        if count % usize::from(channels) != 0 { return None; }
        let samples = bytes[9..].chunks_exact(2)
            .map(|sample| i16::from_be_bytes([sample[0], sample[1]]))
            .collect();
        Some(Self { sample_rate, channels, samples })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn audio_frame_roundtrips_and_rejects_bad_lengths() {
        let frame = AudioFrame { sample_rate: 48_000, channels: 2, samples: vec![-32768, -1, 0, 32767] };
        assert_eq!(AudioFrame::from_bytes(&frame.to_bytes()), Some(frame));
        assert!(AudioFrame::from_bytes(&[0, 0, 0, 0, 2, 0, 0, 0, 0]).is_some());
        assert!(AudioFrame::from_bytes(&[0; 10]).is_none());
    }
}
