//! Relay media transport view over the session's [`ReliableChannel`].

use super::{ReliableSender, Transport, TransportError, TransportReceiver, TransportSender};
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
            Box::new(RelaySender { sender: self.sender }),
            Box::new(RelayReceiver { media_rx: self.media_rx.take().expect("media receiver already taken") }),
        )
    }
}

pub struct RelaySender { sender: ReliableSender }

#[async_trait::async_trait]
impl TransportSender for RelaySender {
    async fn send(&mut self, tag: Tag, sealed: &[u8]) -> Result<(), TransportError> {
        self.sender.send_frame(tag, sealed).await
    }
}

pub struct RelayReceiver { media_rx: mpsc::Receiver<(Tag, Vec<u8>)> }

#[async_trait::async_trait]
impl TransportReceiver for RelayReceiver {
    async fn recv(&mut self) -> Option<(Tag, Vec<u8>)> { self.media_rx.recv().await }
}
