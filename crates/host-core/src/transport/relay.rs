//! Relay media transport view over the session's [`ReliableChannel`].

use super::{reliable::VideoPermit, ReliableSender, Transport, TransportError, TransportReceiver, TransportSender};
use protocol::Tag;
use std::marker::PhantomData;
use tokio::sync::mpsc;

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
            Box::new(RelaySender { sender: self.sender, video_permit: None }),
            Box::new(RelayReceiver { media_rx: self.media_rx.take().expect("media receiver already taken") }),
        )
    }
}

pub struct RelaySender {
    sender: ReliableSender,
    /// Video budget reserved by `ready_for_video` for the next video frame.
    video_permit: Option<VideoPermit>,
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
            // A closed writer surfaces as an error on the next send.
            self.video_permit = self.sender.video_permit().await.ok();
        }
    }
}

pub struct RelayReceiver { media_rx: mpsc::Receiver<(Tag, Vec<u8>)> }

#[async_trait::async_trait]
impl TransportReceiver for RelayReceiver {
    async fn recv(&mut self) -> Option<(Tag, Vec<u8>)> { self.media_rx.recv().await }
}
