use openh264::{
    encoder::{BitRate, Encoder as OpenH264Encoder, EncoderConfig, FrameRate, FrameType},
    formats::{BgraSliceU8, YUVBuffer},
    OpenH264API,
};

#[derive(Clone)]
pub struct RawFrame {
    pub width: u16,
    pub height: u16,
    pub data: Vec<u8>,
    pub format: PixelFormat,
    pub timestamp_us: u32,
}

#[derive(Clone, Copy)]
pub enum PixelFormat {
    Bgra,
}

pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub keyframe: bool,
}

pub trait Encoder {
    fn encode(&mut self, frame: &RawFrame) -> Option<EncodedFrame>;
}

pub struct H264Encoder {
    encoder: OpenH264Encoder,
    first_frame: bool,
    width: u16,
    height: u16,
}

impl H264Encoder {
    pub fn new(width: u16, height: u16, fps: u32, bitrate: u32) -> Self {
        let config = EncoderConfig::new()
            .bitrate(BitRate::from_bps(bitrate))
            .max_frame_rate(FrameRate::from_hz(fps as f32));
        let encoder = OpenH264Encoder::with_api_config(OpenH264API::from_source(), config)
            .expect("openh264 encoder initialization failed");

        Self {
            encoder,
            first_frame: true,
            width,
            height,
        }
    }

    pub fn force_intra_frame(&mut self) {
        self.encoder.force_intra_frame();
    }
}

impl Encoder for H264Encoder {
    fn encode(&mut self, frame: &RawFrame) -> Option<EncodedFrame> {
        if !matches!(frame.format, PixelFormat::Bgra)
            || frame.width != self.width
            || frame.height != self.height
            || frame.data.len() != usize::from(frame.width) * usize::from(frame.height) * 4
        {
            return None;
        }

        let source = BgraSliceU8::new(
            &frame.data,
            (usize::from(frame.width), usize::from(frame.height)),
        );
        let yuv = YUVBuffer::from_rgb_source(source);
        if self.first_frame {
            self.encoder.force_intra_frame();
        }

        let bitstream = self.encoder.encode(&yuv).ok()?;
        let keyframe = self.first_frame || bitstream.frame_type() == FrameType::IDR;
        self.first_frame = false;

        Some(EncodedFrame {
            data: bitstream.to_vec(),
            keyframe,
        })
    }
}

pub fn video_header(seq: u64, ts_us: u32, keyframe: bool, w: u16, h: u16) -> Vec<u8> {
    let mut header = Vec::with_capacity(18);
    header.extend_from_slice(&seq.to_be_bytes());
    header.extend_from_slice(&ts_us.to_be_bytes());
    header.push(1);
    header.push(u8::from(keyframe));
    header.extend_from_slice(&w.to_be_bytes());
    header.extend_from_slice(&h.to_be_bytes());
    header
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_header_is_18_bytes_h264_with_keyframe_flag() {
        let h = video_header(5, 1000, true, 1280, 720);

        assert_eq!(h.len(), 18);
        assert_eq!(&h[0..8], &5u64.to_be_bytes());
        assert_eq!(&h[8..12], &1000u32.to_be_bytes());
        assert_eq!(h[12], 1);
        assert_eq!(h[13] & 1, 1);
        assert_eq!(&h[14..16], &1280u16.to_be_bytes());
        assert_eq!(&h[16..18], &720u16.to_be_bytes());
    }

    #[test]
    fn encoder_produces_keyframe_on_first_frame() {
        let mut enc = H264Encoder::new(64, 64, 15, 1_000_000);
        let frame = RawFrame {
            width: 64,
            height: 64,
            data: vec![120u8; 64 * 64 * 4],
            format: PixelFormat::Bgra,
            timestamp_us: 0,
        };

        let output = enc.encode(&frame).expect("encodes");
        assert!(!output.data.is_empty());
        assert!(output.keyframe, "first frame must be a keyframe");
    }
}
