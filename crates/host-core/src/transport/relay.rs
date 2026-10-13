//! Relay media transport view over the session's [`ReliableChannel`].

use super::link::RttMeter;
use super::rate::{AimdEstimator, RateControl};
use super::{reliable::VideoPermit, ReliableSender, Transport, TransportError, TransportReceiver, TransportSender};
use protocol::Tag;
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// A video frame that waits longer than this for room in the relay send
/// queue means the link is slower than the stream: lower the bitrate.
const CONGESTED_WAIT: Duration = Duration::from_millis(40);

/// A thin media-only view over the reliable relay channel.
pub struct RelayTransport<S = tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    sender: ReliableSender,
    media_rx: Option<mpsc::Receiver<(Tag, Vec<u8>)>>,
    _stream: PhantomData<S>,
}

impl<S> RelayTransport<S> {
    pub fn new(sender: ReliableSender, media_rx: mpsc::Receiver<(Tag, Vec<u8>)>) -> Self {
        Self { sender, media_rx: Some(media_rx), _stream: PhantomData }
    }
}

#[async_trait::async_trait]
impl<S> Transport for RelayTransport<S> {
    fn into_split(mut self) -> (Box<dyn TransportSender>, Box<dyn TransportReceiver>) {
        (
            Box::new(RelaySender {
                sender: self.sender,
                video_permit: None,
                waiting_since: None,
                rate: Arc::new(RateControl::default()),
                rtt: Arc::new(RttMeter::default()),
                estimator: AimdEstimator::default(),
            }),
            Box::new(RelayReceiver { media_rx: self.media_rx.take().expect("media receiver already taken") }),
        )
    }
}

pub struct RelaySender {
    sender: ReliableSender,
    /// Video budget reserved by `ready_for_video` for the next video frame.
    video_permit: Option<VideoPermit>,
    /// When the current wait for a video permit began (kept across
    /// cancelled `ready_for_video` calls).
    waiting_since: Option<Instant>,
    rate: Arc<RateControl>,
    /// Filled by the viewer session from its ping/pong exchange; the
    /// relay transport itself measures nothing.
    rtt: Arc<RttMeter>,
    estimator: AimdEstimator,
}

impl RelaySender {
    /// The relay is TCP: congestion shows as video frames waiting for the
    /// send queue rather than as loss.
    fn queue_sample(&mut self, waited: Duration) {
        let ceiling = self.rate.ceiling();
        if ceiling == 0 {
            return;
        }
        let target = self.estimator.on_queue_sample(Instant::now(), ceiling, waited > CONGESTED_WAIT);
        self.rate.set_target(target);
    }
}

#[async_trait::async_trait]
impl TransportSender for RelaySender {
    async fn send(&mut self, tag: Tag, sealed: &[u8]) -> Result<(), TransportError> {
        if tag != Tag::Video {
            return self.sender.send_frame(tag, sealed).await;
        }
        let permit = match self.video_permit.take() {
            Some(permit) => permit,
            None => self.sender.video_permit().await?,
        };
        self.sender.send_video(permit, sealed).await
    }

    async fn ready_for_video(&mut self) {
        if self.video_permit.is_none() {
            let since = *self.waiting_since.get_or_insert_with(Instant::now);
            // A closed writer surfaces as an error on the next send.
            self.video_permit = self.sender.video_permit().await.ok();
            self.waiting_since = None;
            self.queue_sample(since.elapsed());
        }
    }

    fn rate_control(&self) -> Arc<RateControl> { Arc::clone(&self.rate) }

    fn rtt_meter(&self) -> Arc<RttMeter> { Arc::clone(&self.rtt) }
}

pub struct RelayReceiver { media_rx: mpsc::Receiver<(Tag, Vec<u8>)> }

#[async_trait::async_trait]
impl TransportReceiver for RelayReceiver {
    async fn recv(&mut self) -> Option<(Tag, Vec<u8>)> { self.media_rx.recv().await }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::ReliableChannel;
    use tokio::io::AsyncReadExt;
    use tokio_tungstenite::{tungstenite::protocol::Role, WebSocketStream};

    const CEILING: u32 = 12_000_000;

    /// A relay sender whose far end only starts reading after `stall`.
    async fn sender_with_stalled_peer(stall: Duration) -> (Box<dyn TransportSender>, ReliableChannel<tokio::io::DuplexStream>) {
        let (client, mut server) = tokio::io::duplex(4 * 1024);
        tokio::spawn(async move {
            tokio::time::sleep(stall).await;
            let mut buf = vec![0u8; 64 * 1024];
            while matches!(server.read(&mut buf).await, Ok(n) if n > 0) {}
        });
        let ws = WebSocketStream::from_raw_socket(client, Role::Client, None).await;
        let mut channel = ReliableChannel::spawn(ws);
        let relay: RelayTransport<tokio::io::DuplexStream> = RelayTransport::new(channel.sender(), channel.take_media_rx().unwrap());
        let (sender, _receiver) = relay.into_split();
        sender.rate_control().set_ceiling(CEILING);
        // The channel owns the socket tasks; the caller keeps it alive.
        (sender, channel)
    }

    async fn send_frames(sender: &mut Box<dyn TransportSender>, count: usize) {
        let frame = vec![0u8; 32 * 1024];
        for _ in 0..count {
            sender.ready_for_video().await;
            sender.send(Tag::Video, &frame).await.unwrap();
        }
    }

    #[tokio::test]
    async fn a_backed_up_send_queue_lowers_the_target_bitrate() {
        let (mut sender, _channel) = sender_with_stalled_peer(Duration::from_millis(150)).await;
        // Two frames fit in flight; the third waits for the stalled socket.
        send_frames(&mut sender, 3).await;
        let target = sender.rate_control().target();
        assert!(target < CEILING, "target {target}");
    }

    #[tokio::test]
    async fn a_draining_send_queue_keeps_the_ceiling() {
        let (mut sender, _channel) = sender_with_stalled_peer(Duration::ZERO).await;
        send_frames(&mut sender, 5).await;
        assert_eq!(sender.rate_control().target(), CEILING);
    }
}
