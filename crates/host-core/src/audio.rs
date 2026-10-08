use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Sample;
use opus::{Application, Channels, Encoder};
use protocol::audio::AudioFrame;
use tokio::sync::mpsc;

const SAMPLE_RATE: u32 = 48_000;
const FRAME_DURATION_MS: u16 = 20;
const SAMPLES_PER_CHANNEL: usize = 960;

/// Capture the host's audio as 48 kHz, 20 ms Opus. Prefers system output
/// (what the host hears) and falls back to the default microphone where
/// loopback isn't available: macOS before 14.6, Linux, or a failed tap.
/// Devices at other rates or with more than two channels are converted by
/// `PcmConverter`.
pub fn spawn_capture() -> mpsc::Receiver<AudioFrame> {
    let (tx, rx) = mpsc::channel(8);
    std::thread::spawn(move || {
        let host = cpal::default_host();
        let Some(stream) = system_audio_stream(&host, &tx).or_else(|| microphone_stream(&host, &tx)) else {
            log::warn!("audio capture unavailable: no usable output loopback or input device");
            return;
        };
        while !tx.is_closed() { std::thread::sleep(std::time::Duration::from_millis(250)); }
        drop(stream);
    });
    rx
}

/// Loopback capture of the default output device. cpal implements this for
/// an input stream built on an output device (CoreAudio tap on macOS 14.6+,
/// WASAPI loopback on Windows).
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn system_audio_stream(host: &cpal::Host, tx: &mpsc::Sender<AudioFrame>) -> Option<cpal::Stream> {
    let device = host.default_output_device()?;
    let config = device.default_output_config().ok()?;
    let stream = open_stream(&device, config, tx);
    if stream.is_some() { log::info!("audio source: system output"); }
    stream
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn system_audio_stream(_host: &cpal::Host, _tx: &mpsc::Sender<AudioFrame>) -> Option<cpal::Stream> {
    None
}

fn microphone_stream(host: &cpal::Host, tx: &mpsc::Sender<AudioFrame>) -> Option<cpal::Stream> {
    let device = host.default_input_device()?;
    let config = device.default_input_config().ok()?;
    let stream = open_stream(&device, config, tx);
    if stream.is_some() { log::info!("audio source: microphone"); }
    stream
}

fn open_stream(device: &cpal::Device, supported: cpal::SupportedStreamConfig,
    tx: &mpsc::Sender<AudioFrame>) -> Option<cpal::Stream> {
    let config: cpal::StreamConfig = supported.into();
    if config.channels == 0 { return None; }
    let converter = PcmConverter::new(config.sample_rate, usize::from(config.channels));
    let error = |error: cpal::Error| log::warn!("audio stream error: {error}");
    let tx = tx.clone();
    let stream = match supported.sample_format() {
        cpal::SampleFormat::I8 => build::<i8, _>(device, &config, converter, tx, error),
        cpal::SampleFormat::I16 => build::<i16, _>(device, &config, converter, tx, error),
        cpal::SampleFormat::I32 => build::<i32, _>(device, &config, converter, tx, error),
        cpal::SampleFormat::I64 => build::<i64, _>(device, &config, converter, tx, error),
        cpal::SampleFormat::U8 => build::<u8, _>(device, &config, converter, tx, error),
        cpal::SampleFormat::U16 => build::<u16, _>(device, &config, converter, tx, error),
        cpal::SampleFormat::U32 => build::<u32, _>(device, &config, converter, tx, error),
        cpal::SampleFormat::U64 => build::<u64, _>(device, &config, converter, tx, error),
        cpal::SampleFormat::F32 => build::<f32, _>(device, &config, converter, tx, error),
        cpal::SampleFormat::F64 => build::<f64, _>(device, &config, converter, tx, error),
        _ => return None,
    };
    let stream = stream.map_err(|error| log::debug!("audio stream build failed: {error}")).ok()?;
    stream.play().ok()?;
    Some(stream)
}

fn build<T, E>(device: &cpal::Device, config: &cpal::StreamConfig, mut converter: PcmConverter,
    tx: mpsc::Sender<AudioFrame>, error: E) -> Result<cpal::Stream, cpal::Error>
where
    T: cpal::Sample + cpal::SizedSample,
    i16: cpal::FromSample<T>,
    E: FnMut(cpal::Error) + Send + 'static,
{
    let channels = converter.out_channels() as u8;
    let opus_channels = if channels == 1 { Channels::Mono } else { Channels::Stereo };
    let mut encoder = Encoder::new(SAMPLE_RATE, opus_channels, Application::Audio)
        .map_err(|error| cpal::Error::with_message(cpal::ErrorKind::BackendError, format!("opus encoder: {error}")))?;
    let frame_len = SAMPLES_PER_CHANNEL * usize::from(channels);
    let mut pending = Vec::<i16>::with_capacity(frame_len * 2);
    let mut raw = Vec::<i16>::new();
    device.build_input_stream(*config, move |data: &[T], _| {
        raw.clear();
        raw.extend(data.iter().map(|sample| i16::from_sample(*sample)));
        converter.push(&raw, &mut pending);
        while pending.len() >= frame_len {
            let pcm: Vec<i16> = pending.drain(..frame_len).collect();
            let mut encoded = [0u8; 4_000];
            if let Ok(length) = encoder.encode(&pcm, &mut encoded) {
                let frame = AudioFrame { sample_rate: SAMPLE_RATE, channels, frame_duration_ms: FRAME_DURATION_MS, payload: encoded[..length].to_vec() };
                let _ = tx.try_send(frame);
            }
        }
    }, error, None)
}

/// Streams interleaved PCM at any device rate/channel count into 48 kHz mono
/// (1-channel input) or stereo (first two channels of anything wider), using
/// linear interpolation. Position is tracked as an exact integer so output
/// does not depend on how the device chunks its callbacks.
pub(crate) struct PcmConverter {
    in_rate: u64,
    in_channels: usize,
    out_channels: usize,
    /// Downmixed input frames not yet consumed, interleaved by `out_channels`.
    buf: Vec<i16>,
    /// Next output position in input frames, scaled by `SAMPLE_RATE`.
    pos: u64,
}

impl PcmConverter {
    pub(crate) fn new(in_rate: u32, in_channels: usize) -> Self {
        let out_channels = if in_channels == 1 { 1 } else { 2 };
        Self { in_rate: u64::from(in_rate), in_channels, out_channels, buf: Vec::new(), pos: 0 }
    }

    pub(crate) fn out_channels(&self) -> usize { self.out_channels }

    pub(crate) fn push(&mut self, input: &[i16], out: &mut Vec<i16>) {
        for frame in input.chunks_exact(self.in_channels) {
            self.buf.extend_from_slice(&frame[..self.out_channels]);
        }
        let ch = self.out_channels;
        let frames = (self.buf.len() / ch) as u64;
        let scale = u64::from(SAMPLE_RATE);
        while self.pos / scale + 1 < frames {
            let idx = (self.pos / scale) as usize;
            let frac = (self.pos % scale) as i64;
            for c in 0..ch {
                let a = i64::from(self.buf[idx * ch + c]);
                let b = i64::from(self.buf[(idx + 1) * ch + c]);
                out.push(((a * (scale as i64 - frac) + b * frac) / scale as i64) as i16);
            }
            self.pos += self.in_rate;
        }
        let consumed = (self.pos / scale) as usize;
        self.buf.drain(..consumed * ch);
        self.pos -= consumed as u64 * scale;
    }
}

#[cfg(test)]
mod tests {
    use super::PcmConverter;

    #[test]
    fn passthrough_at_48k_stereo() {
        let mut c = PcmConverter::new(48_000, 2);
        let input: Vec<i16> = (0..960 * 2).map(|i| i as i16).collect();
        let mut out = Vec::new();
        c.push(&input, &mut out);
        // The last frame is held back as the interpolation anchor.
        assert_eq!(out, input[..input.len() - 2]);
        assert_eq!(c.out_channels(), 2);
    }

    #[test]
    fn resamples_44k1_mono_to_48k() {
        let mut c = PcmConverter::new(44_100, 1);
        let input = vec![1000i16; 44_100];
        let mut out = Vec::new();
        c.push(&input, &mut out);
        assert!((47_995..=48_000).contains(&out.len()), "got {}", out.len());
        assert!(out.iter().all(|&s| s == 1000));
        assert_eq!(c.out_channels(), 1);
    }

    #[test]
    fn multichannel_keeps_first_two_channels() {
        let mut c = PcmConverter::new(48_000, 4);
        let input: Vec<i16> = [1, 2, 3, 4].repeat(10);
        let mut out = Vec::new();
        c.push(&input, &mut out);
        assert_eq!(out, [1, 2].repeat(9));
        assert_eq!(c.out_channels(), 2);
    }

    #[test]
    fn chunked_input_matches_single_push() {
        let input: Vec<i16> = (0..4410).map(|i| ((i * 37) % 2000) as i16).collect();
        let mut whole = Vec::new();
        PcmConverter::new(44_100, 1).push(&input, &mut whole);
        let mut chunked = Vec::new();
        let mut c = PcmConverter::new(44_100, 1);
        for chunk in input.chunks(441) { c.push(chunk, &mut chunked); }
        assert_eq!(whole, chunked);
    }

    #[test]
    fn interpolates_between_samples() {
        let mut c = PcmConverter::new(24_000, 1);
        let mut out = Vec::new();
        c.push(&[0, 100, 200], &mut out);
        assert_eq!(out, [0, 50, 100, 150]);
    }
}
