use cpal::Sample;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use protocol::audio::AudioFrame;
use tokio::sync::mpsc;

/// Capture the default input device on its own OS thread. CPAL streams stay on
/// this thread because some platform stream handles are not Send. The input
/// device is also the safe fallback when a platform cannot expose loopback.
pub fn spawn_capture() -> mpsc::Receiver<AudioFrame> {
    let (tx, rx) = mpsc::channel(8);
    std::thread::spawn(move || {
        let host = cpal::default_host();
        let Some(device) = host.default_input_device() else { return };
        let Ok(default_config) = device.default_input_config() else { return };
        let config: cpal::StreamConfig = default_config.clone().into();
        let sample_rate = config.sample_rate.0;
        let channels = config.channels as u8;
        if channels == 0 { return; }
        let error = |_error| {};
        let stream = match default_config.sample_format() {
            cpal::SampleFormat::I8 => build::<i8, _>(&device, &config, sample_rate, channels, tx.clone(), error),
            cpal::SampleFormat::I16 => build::<i16, _>(&device, &config, sample_rate, channels, tx.clone(), error),
            cpal::SampleFormat::I32 => build::<i32, _>(&device, &config, sample_rate, channels, tx.clone(), error),
            cpal::SampleFormat::I64 => build::<i64, _>(&device, &config, sample_rate, channels, tx.clone(), error),
            cpal::SampleFormat::U8 => build::<u8, _>(&device, &config, sample_rate, channels, tx.clone(), error),
            cpal::SampleFormat::U16 => build::<u16, _>(&device, &config, sample_rate, channels, tx.clone(), error),
            cpal::SampleFormat::U32 => build::<u32, _>(&device, &config, sample_rate, channels, tx.clone(), error),
            cpal::SampleFormat::U64 => build::<u64, _>(&device, &config, sample_rate, channels, tx.clone(), error),
            cpal::SampleFormat::F32 => build::<f32, _>(&device, &config, sample_rate, channels, tx.clone(), error),
            cpal::SampleFormat::F64 => build::<f64, _>(&device, &config, sample_rate, channels, tx.clone(), error),
            _ => return,
        };
        let Ok(stream) = stream else { return };
        if stream.play().is_err() { return; }
        while !tx.is_closed() { std::thread::sleep(std::time::Duration::from_millis(250)); }
    });
    rx
}

fn build<T, E>(device: &cpal::Device, config: &cpal::StreamConfig, rate: u32, channels: u8,
    tx: mpsc::Sender<AudioFrame>, error: E) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: cpal::Sample + cpal::SizedSample,
    i16: cpal::FromSample<T>,
    E: FnMut(cpal::StreamError) + Send + 'static,
{
    device.build_input_stream(config, move |data: &[T], _| {
        let samples = data.iter().map(|sample| i16::from_sample(*sample)).collect();
        let _ = tx.try_send(AudioFrame { sample_rate: rate, channels, samples });
    }, error, None)
}
