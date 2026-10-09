//! H.264 video encoding.
//!
//! Bitstream contract (what `viewer/src/lib/decode.ts` feeds to WebCodecs,
//! configured as `avc1.42E01E` with no `description`):
//! - Annex B byte stream: every NAL unit is prefixed with a `00 00 00 01`
//!   start code (no AVCC length prefixes).
//! - Keyframes are IDR access units that carry SPS and PPS in-band, in front
//!   of the IDR slice, so a decoder can start from any keyframe.
//! - Baseline profile, no B-frames: one access unit out per frame in.
//! - `EncodedFrame::keyframe` is true exactly when the access unit holds an
//!   IDR slice; it becomes the WebCodecs chunk `type`.
//!
//! [`VideoEncoder`] picks the best backend for the platform and every backend
//! honours this contract.

#[cfg(target_os = "macos")]
mod videotoolbox;

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

    /// Request that the next encoded frame be an intra frame when supported.
    fn force_intra_frame(&mut self) {}
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
    fn force_intra_frame(&mut self) {
        self.encoder.force_intra_frame();
    }

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

/// Annex B start code written in front of every NAL unit.
#[cfg(any(target_os = "macos", test))]
const START_CODE: [u8; 4] = [0, 0, 0, 1];
/// `nal_unit_type` of an IDR slice.
#[cfg(any(target_os = "macos", test))]
const NAL_IDR: u8 = 5;

/// Converts length-prefixed (AVCC / ISO 14496-15) NAL units into Annex B.
///
/// `length_size` is the size in bytes (1, 2 or 4) of each big-endian NAL
/// length prefix. The converted NAL units are appended to `out`. Returns
/// whether an IDR slice was present, or `None` if `avcc` is malformed (in
/// which case `out` may hold a partial conversion).
#[cfg(any(target_os = "macos", test))]
pub(crate) fn avcc_to_annex_b(avcc: &[u8], length_size: usize, out: &mut Vec<u8>) -> Option<bool> {
    if !matches!(length_size, 1 | 2 | 4) {
        return None;
    }
    let mut idr = false;
    let mut rest = avcc;
    while !rest.is_empty() {
        if rest.len() < length_size {
            return None;
        }
        let (prefix, tail) = rest.split_at(length_size);
        let len = prefix
            .iter()
            .fold(0usize, |len, &byte| (len << 8) | usize::from(byte));
        if len == 0 || tail.len() < len {
            return None;
        }
        let (nal, tail) = tail.split_at(len);
        idr |= nal[0] & 0x1f == NAL_IDR;
        out.extend_from_slice(&START_CODE);
        out.extend_from_slice(nal);
        rest = tail;
    }
    Some(idr)
}

/// Appends parameter sets (SPS, PPS, ...) to `out` as Annex B NAL units.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn append_parameter_sets<'a>(
    sets: impl IntoIterator<Item = &'a [u8]>,
    out: &mut Vec<u8>,
) {
    for set in sets {
        out.extend_from_slice(&START_CODE);
        out.extend_from_slice(set);
    }
}

/// Which implementation a [`VideoEncoder`] is using.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncoderBackend {
    /// openh264, in software. Available everywhere.
    OpenH264,
    /// Apple VideoToolbox, hardware accelerated (macOS).
    VideoToolbox,
}

impl std::fmt::Display for EncoderBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::OpenH264 => "openh264 (software)",
            Self::VideoToolbox => "VideoToolbox (hardware)",
        })
    }
}

enum Backend {
    OpenH264(Box<H264Encoder>),
    #[cfg(target_os = "macos")]
    VideoToolbox(videotoolbox::VtEncoder),
}

/// H.264 encoder that uses the best backend available on this machine.
///
/// On macOS it tries a hardware VideoToolbox session and falls back to
/// openh264 if the session cannot be created, or if it fails later (the
/// fallback then starts with a keyframe). Other platforms use openh264; new
/// hardware backends (Media Foundation, VAAPI) slot in as more `Backend`
/// variants. Setting `OPENDESK_ENCODER=software` forces openh264.
// The settings are only read when rebuilding after a hardware failure.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub struct VideoEncoder {
    backend: Backend,
    width: u16,
    height: u16,
    fps: u32,
    bitrate: u32,
}

impl VideoEncoder {
    pub fn best_available(width: u16, height: u16, fps: u32, bitrate: u32) -> Self {
        let force_software = std::env::var("OPENDESK_ENCODER")
            .is_ok_and(|value| value.eq_ignore_ascii_case("software"));
        let backend = if force_software {
            log::info!("OPENDESK_ENCODER=software: hardware encoding disabled");
            None
        } else {
            Self::hardware(width, height, fps, bitrate)
        };
        let backend = backend.unwrap_or_else(|| {
            Backend::OpenH264(Box::new(H264Encoder::new(width, height, fps, bitrate)))
        });
        let encoder = Self {
            backend,
            width,
            height,
            fps,
            bitrate,
        };
        log::info!(
            "video encoder: {} {width}x{height} @ {fps} fps, {bitrate} bps",
            encoder.backend()
        );
        encoder
    }

    #[cfg(target_os = "macos")]
    fn hardware(width: u16, height: u16, fps: u32, bitrate: u32) -> Option<Backend> {
        match videotoolbox::VtEncoder::new(width, height, fps, bitrate) {
            Ok(encoder) => Some(Backend::VideoToolbox(encoder)),
            Err(error) => {
                log::info!("VideoToolbox unavailable ({error}); falling back to openh264");
                None
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn hardware(_width: u16, _height: u16, _fps: u32, _bitrate: u32) -> Option<Backend> {
        None
    }

    pub fn backend(&self) -> EncoderBackend {
        match self.backend {
            Backend::OpenH264(_) => EncoderBackend::OpenH264,
            #[cfg(target_os = "macos")]
            Backend::VideoToolbox(_) => EncoderBackend::VideoToolbox,
        }
    }
}

impl Encoder for VideoEncoder {
    fn encode(&mut self, frame: &RawFrame) -> Option<EncodedFrame> {
        match &mut self.backend {
            Backend::OpenH264(encoder) => encoder.encode(frame),
            #[cfg(target_os = "macos")]
            Backend::VideoToolbox(encoder) => match encoder.encode(frame) {
                Ok(encoded) => encoded,
                Err(error) => {
                    log::warn!("VideoToolbox encode failed ({error}); switching to openh264");
                    // A fresh openh264 encoder always starts with a keyframe.
                    let mut software =
                        H264Encoder::new(self.width, self.height, self.fps, self.bitrate);
                    let encoded = software.encode(frame);
                    self.backend = Backend::OpenH264(Box::new(software));
                    encoded
                }
            },
        }
    }

    fn force_intra_frame(&mut self) {
        match &mut self.backend {
            Backend::OpenH264(encoder) => encoder.force_intra_frame(),
            #[cfg(target_os = "macos")]
            Backend::VideoToolbox(encoder) => encoder.force_intra_frame(),
        }
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
pub(crate) mod tests {
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

    /// `nal_unit_type`s of an Annex B stream with 4-byte start codes.
    pub(crate) fn nal_types(annex_b: &[u8]) -> Vec<u8> {
        let mut types = Vec::new();
        let mut i = 0;
        while i + 4 < annex_b.len() {
            if annex_b[i..i + 4] == START_CODE {
                types.push(annex_b[i + 4] & 0x1f);
                i += 4;
            } else {
                i += 1;
            }
        }
        types
    }

    /// A moving BGRA test pattern so that delta frames carry real changes.
    pub(crate) fn synthetic_frame(width: u16, height: u16, index: u32) -> RawFrame {
        let (w, h) = (usize::from(width), usize::from(height));
        let mut data = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                let shifted = x + index as usize * 4;
                data.extend_from_slice(&[
                    (shifted % 256) as u8,
                    (y % 256) as u8,
                    ((shifted ^ y) % 256) as u8,
                    255,
                ]);
            }
        }
        RawFrame {
            width,
            height,
            data,
            format: PixelFormat::Bgra,
            timestamp_us: index * 66_666,
        }
    }

    #[test]
    fn openh264_output_is_annex_b_with_parameter_sets_on_keyframes() {
        let mut enc = H264Encoder::new(64, 64, 15, 1_000_000);
        let key = enc.encode(&synthetic_frame(64, 64, 0)).expect("encodes");
        assert_eq!(key.data[..4], START_CODE);
        let types = nal_types(&key.data);
        assert_eq!(
            types[..2],
            [7, 8],
            "SPS and PPS lead the keyframe: {types:?}"
        );
        assert!(types.contains(&NAL_IDR));

        let delta = enc.encode(&synthetic_frame(64, 64, 1)).expect("encodes");
        assert!(!delta.keyframe);
        assert!(!nal_types(&delta.data)
            .iter()
            .any(|t| matches!(t, 5 | 7 | 8)));
    }

    #[test]
    fn avcc_to_annex_b_replaces_length_prefixes_and_detects_idr() {
        let avcc = [0, 0, 0, 2, 0x06, 0xAA, 0, 0, 0, 3, 0x65, 0x88, 0x84];
        let mut out = Vec::new();
        assert_eq!(avcc_to_annex_b(&avcc, 4, &mut out), Some(true));
        assert_eq!(out, [0, 0, 0, 1, 0x06, 0xAA, 0, 0, 0, 1, 0x65, 0x88, 0x84]);

        let mut out = Vec::new();
        assert_eq!(
            avcc_to_annex_b(&[0, 2, 0x41, 0x9A], 2, &mut out),
            Some(false)
        );
        assert_eq!(out, [0, 0, 0, 1, 0x41, 0x9A]);

        let mut out = Vec::new();
        assert_eq!(avcc_to_annex_b(&[1, 0x65], 1, &mut out), Some(true));
        assert_eq!(out, [0, 0, 0, 1, 0x65]);

        let mut out = Vec::new();
        assert_eq!(avcc_to_annex_b(&[], 4, &mut out), Some(false));
        assert!(out.is_empty());
    }

    #[test]
    fn avcc_to_annex_b_rejects_malformed_input() {
        let mut out = Vec::new();
        // NAL length runs past the end of the buffer.
        assert_eq!(
            avcc_to_annex_b(&[0, 0, 0, 5, 0x65, 0x88], 4, &mut out),
            None
        );
        // Truncated length prefix.
        assert_eq!(
            avcc_to_annex_b(&[0, 0, 0, 1, 0x41, 0, 0], 4, &mut out),
            None
        );
        // Zero-length NAL unit.
        assert_eq!(avcc_to_annex_b(&[0, 0, 0, 0], 4, &mut out), None);
        // Unsupported prefix size.
        assert_eq!(avcc_to_annex_b(&[0, 0, 1, 0x65], 3, &mut out), None);
    }

    #[test]
    fn parameter_sets_are_written_with_start_codes() {
        let mut out = Vec::new();
        append_parameter_sets([&[0x67, 0x42][..], &[0x68, 0xCE][..]], &mut out);
        assert_eq!(out, [0, 0, 0, 1, 0x67, 0x42, 0, 0, 0, 1, 0x68, 0xCE]);
    }

    #[test]
    fn best_available_encoder_starts_with_a_keyframe() {
        let mut enc = VideoEncoder::best_available(128, 96, 15, 1_000_000);
        let output = enc.encode(&synthetic_frame(128, 96, 0)).expect("encodes");
        assert!(
            output.keyframe,
            "{} must start with a keyframe",
            enc.backend()
        );
        assert_eq!(nal_types(&output.data)[..2], [7, 8]);
    }
}
