// Host system audio through cpal and Opus. Phones send no audio yet: their
// sessions get a closed channel, which the media task reads as "no audio".
#[cfg(not(target_os = "android"))]
mod desktop;
#[cfg(not(target_os = "android"))]
pub use desktop::spawn_capture;

#[cfg(target_os = "android")]
pub fn spawn_capture() -> tokio::sync::mpsc::Receiver<protocol::audio::AudioFrame> {
    tokio::sync::mpsc::channel(1).1
}
