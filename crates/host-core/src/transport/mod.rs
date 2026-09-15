//! Transport abstractions for carrying sealed protocol frames.

pub mod relay;

pub use relay::RelayTransport;

/// Errors encountered while sending a sealed frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    /// The peer or transport has closed the connection.
    Closed,
    /// An I/O or WebSocket error, represented without exposing a concrete
    /// transport implementation in the trait API.
    Io(String),
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed => f.write_str("transport is closed"),
            Self::Io(message) => write!(f, "transport I/O error: {message}"),
        }
    }
}

impl std::error::Error for TransportError {}

/// A transport for sealed, tagged protocol frames.
#[async_trait::async_trait]
pub trait Transport {
    async fn send(&mut self, tag: protocol::Tag, sealed: &[u8]) -> Result<(), TransportError>;

    async fn recv(&mut self) -> Option<(protocol::Tag, Vec<u8>)>;
}

/// Transport implementations available before the direct UDP path is added.
pub enum AnyTransport {
    Relay(relay::RelayTransport),
}

#[async_trait::async_trait]
impl Transport for AnyTransport {
    async fn send(&mut self, tag: protocol::Tag, sealed: &[u8]) -> Result<(), TransportError> {
        match self {
            Self::Relay(transport) => transport.send(tag, sealed).await,
        }
    }

    async fn recv(&mut self) -> Option<(protocol::Tag, Vec<u8>)> {
        match self {
            Self::Relay(transport) => transport.recv().await,
        }
    }
}
