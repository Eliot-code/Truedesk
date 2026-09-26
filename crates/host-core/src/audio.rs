use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Sample;
use opus::{Application, Channels, Encoder};
use protocol::audio::AudioFrame;
use tokio::sync::mpsc;

const SAMPLE_RATE: u32 = 48_000;
const FRAME_DURATION_MS: u16 = 20;
const SAMPLES_PER_CHANNEL: usize = 960;

/// Capture and packetize the default input as 48 kHz, 20 ms Opus. The default
/// input remains the safe fallback on platforms without system-audio loopback.
pub fn spawn_capture() -> mpsc::Receiver<AudioFrame> {
    let (tx, rx) = mpsc::channel(8);
    std::thread::spawn(move || {
        let host = cpal::default_host();
        let Some(device) = host.default_input_device() else { return };
        let Ok(default_config) = device.default_input_config() else { return };
        let config: cpal::StreamConfig = default_config.clone().into();
        let channels = config.channels as u8;
        if config.sample_rate.0 != SAMPLE_RATE || !(channels == 1 || channels == 2) { return; }
        let error = |_error| {};
        let stream = match default_config.sample_format() {
            cpal::SampleFormat::I8 => build::<i8, _>(&device, &config, channels, tx.clone(), error),
            cpal::SampleFormat::I16 => build::<i16, _>(&device, &config, channels, tx.clone(), error),
            cpal::SampleFormat::I32 => build::<i32, _>(&device, &config, channels, tx.clone(), error),
            cpal::SampleFormat::I64 => build::<i64, _>(&device, &config, channels, tx.clone(), error),
            cpal::SampleFormat::U8 => build::<u8, _>(&device, &config, channels, tx.clone(), error),
            cpal::SampleFormat::U16 => build::<u16, _>(&device, &config, channels, tx.clone(), error),
            cpal::SampleFormat::U32 => build::<u32, _>(&device, &config, channels, tx.clone(), error),
            cpal::SampleFormat::U64 => build::<u64, _>(&device, &config, channels, tx.clone(), error),
            cpal::SampleFormat::F32 => build::<f32, _>(&device, &config, channels, tx.clone(), error),
            cpal::SampleFormat::F64 => build::<f64, _>(&device, &config, channels, tx.clone(), error),
            _ => return,
        };
        let Ok(stream) = stream else { return };
        if stream.play().is_err() { return; }
        while !tx.is_closed() { std::thread::sleep(std::time::Duration::from_millis(250)); }
    });
    rx
}

fn build<T, E>(device: &cpal::Device, config: &cpal::StreamConfig, channels: u8,
    tx: mpsc::Sender<AudioFrame>, error: E) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: cpal::Sample + cpal::SizedSample,
    i16: cpal::FromSample<T>,
    E: FnMut(cpal::StreamError) + Send + 'static,
{
    let opus_channels = if channels == 1 { Channels::Mono } else { Channels::Stereo };
    let mut encoder = Encoder::new(SAMPLE_RATE, opus_channels, Application::Audio)
        .map_err(|_| cpal::BuildStreamError::StreamConfigNotSupported)?;
    let frame_len = SAMPLES_PER_CHANNEL * usize::from(channels);
    let mut pending = Vec::<i16>::with_capacity(frame_len * 2);
    device.build_input_stream(config, move |data: &[T], _| {
        pending.extend(data.iter().map(|sample| i16::from_sample(*sample)));
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
