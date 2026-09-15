//! Direct UDP transport over a nominated ICE connection.

use super::{Transport, TransportError, TransportReceiver, TransportSender};
use protocol::{frame, Tag};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use webrtc_util::conn::Conn;

const FRAG_HEADER_LEN: usize = 8;
/// Keep UDP datagrams comfortably below the usual Internet path MTU.
const FRAG_PAYLOAD: usize = 1100;
const MAX_IN_FLIGHT_MESSAGES: usize = 8;

/// Carries sealed protocol frames over application-level UDP fragmentation.
pub struct DirectTransport {
    pub(crate) conn: Arc<dyn Conn + Send + Sync>,
}

pub struct DirectSender {
    conn: Arc<dyn Conn + Send + Sync>,
    next_msg_id: u32,
}

pub struct DirectReceiver {
    conn: Arc<dyn Conn + Send + Sync>,
    reassembler: Reassembler,
}

impl DirectTransport {
    pub(crate) fn new(conn: Arc<dyn Conn + Send + Sync>) -> Self {
        Self { conn }
    }
}

#[async_trait::async_trait]
impl Transport for DirectTransport {
    fn into_split(self) -> (Box<dyn TransportSender>, Box<dyn TransportReceiver>) {
        (
            Box::new(DirectSender {
                conn: Arc::clone(&self.conn),
                next_msg_id: 0,
            }),
            Box::new(DirectReceiver {
                conn: self.conn,
                reassembler: Reassembler::new(MAX_IN_FLIGHT_MESSAGES),
            }),
        )
    }
}

#[async_trait::async_trait]
impl TransportSender for DirectSender {
    async fn send(&mut self, tag: Tag, sealed: &[u8]) -> Result<(), TransportError> {
        let message = frame::wrap(tag, sealed);
        let fragments = fragment_message(&message, self.next_msg_id)
            .map_err(|error| TransportError::Io(error.to_string()))?;
        self.next_msg_id = self.next_msg_id.wrapping_add(1);

        for datagram in fragments {
            let sent = self
                .conn
                .send(&datagram)
                .await
                .map_err(|error| TransportError::Io(error.to_string()))?;
            if sent != datagram.len() {
                return Err(TransportError::Io(format!(
                    "UDP send was incomplete: sent {sent} of {} bytes",
                    datagram.len()
                )));
            }
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl TransportReceiver for DirectReceiver {
    async fn recv(&mut self) -> Option<(Tag, Vec<u8>)> {
        let mut buf = vec![0u8; 65_535];
        loop {
            let size = self.conn.recv(&mut buf).await.ok()?;
            let Some(message) = self.reassembler.push(&buf[..size]) else {
                continue;
            };
            let Some((tag, body)) = frame::unwrap(&message) else {
                continue;
            };
            return Some((tag, body.to_vec()));
        }
    }
}

/// Fragment a complete wrapped frame. The header uses network byte order.
fn fragment_message(message: &[u8], msg_id: u32) -> Result<Vec<Vec<u8>>, &'static str> {
    if message.is_empty() {
        return Err("cannot fragment an empty message");
    }
    let frag_count = message.len().div_ceil(FRAG_PAYLOAD);
    let frag_count = u16::try_from(frag_count).map_err(|_| "message has too many fragments")?;
    let mut fragments = Vec::with_capacity(frag_count as usize);

    for (index, chunk) in message.chunks(FRAG_PAYLOAD).enumerate() {
        let mut datagram = Vec::with_capacity(FRAG_HEADER_LEN + chunk.len());
        datagram.extend_from_slice(&msg_id.to_be_bytes());
        datagram.extend_from_slice(&(index as u16).to_be_bytes());
        datagram.extend_from_slice(&frag_count.to_be_bytes());
        datagram.extend_from_slice(chunk);
        fragments.push(datagram);
    }
    Ok(fragments)
}

struct InFlightMessage {
    frag_count: u16,
    fragments: Vec<Option<Vec<u8>>>,
    received: usize,
}

/// Reassembles fragments without retransmission. A missing fragment means the
/// frame is discarded when its slot is evicted; video recovers on a keyframe.
struct Reassembler {
    max_in_flight: usize,
    messages: HashMap<u32, InFlightMessage>,
    oldest: VecDeque<u32>,
}

impl Reassembler {
    fn new(max_in_flight: usize) -> Self {
        Self {
            max_in_flight,
            messages: HashMap::new(),
            oldest: VecDeque::new(),
        }
    }

    fn push(&mut self, datagram: &[u8]) -> Option<Vec<u8>> {
        if datagram.len() < FRAG_HEADER_LEN || datagram.len() > FRAG_HEADER_LEN + FRAG_PAYLOAD {
            return None;
        }
        let msg_id = u32::from_be_bytes(datagram[0..4].try_into().ok()?);
        let frag_index = u16::from_be_bytes(datagram[4..6].try_into().ok()?);
        let frag_count = u16::from_be_bytes(datagram[6..8].try_into().ok()?);
        if frag_count == 0 || frag_index >= frag_count {
            return None;
        }

        if !self.messages.contains_key(&msg_id) {
            self.evict_if_full();
            self.messages.insert(
                msg_id,
                InFlightMessage {
                    frag_count,
                    fragments: (0..frag_count).map(|_| None).collect(),
                    received: 0,
                },
            );
            self.oldest.push_back(msg_id);
        }

        let message = self.messages.get_mut(&msg_id)?;
        if message.frag_count != frag_count {
            return None;
        }
        let slot = &mut message.fragments[frag_index as usize];
        if slot.is_some() {
            return None;
        }
        *slot = Some(datagram[FRAG_HEADER_LEN..].to_vec());
        message.received += 1;
        if message.received != message.frag_count as usize {
            return None;
        }

        let message = self.messages.remove(&msg_id)?;
        self.oldest.retain(|&id| id != msg_id);
        let total_len: usize = message
            .fragments
            .iter()
            .filter_map(|fragment| fragment.as_ref().map(Vec::len))
            .sum();
        let mut assembled = Vec::with_capacity(total_len);
        for fragment in message.fragments {
            assembled.extend_from_slice(&fragment?);
        }
        Some(assembled)
    }

    fn evict_if_full(&mut self) {
        if self.messages.len() < self.max_in_flight {
            return;
        }
        while let Some(oldest) = self.oldest.pop_front() {
            if self.messages.remove(&oldest).is_some() {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use webrtc_util::conn::conn_pipe::pipe;

    #[tokio::test]
    async fn loopback_round_trip_preserves_tag_and_sealed_bytes() {
        let (left, right) = pipe();

        let (mut sender, _) = DirectTransport::new(Arc::new(left)).into_split();
        let (_, mut receiver) = DirectTransport::new(Arc::new(right)).into_split();
        let sealed = b"already sealed bytes";

        sender.send(Tag::Video, sealed).await.unwrap();
        assert_eq!(receiver.recv().await, Some((Tag::Video, sealed.to_vec())));
    }

    #[tokio::test]
    async fn loopback_round_trip_reassembles_large_message() {
        let (left, right) = pipe();
        let (mut sender, _) = DirectTransport::new(Arc::new(left)).into_split();
        let (_, mut receiver) = DirectTransport::new(Arc::new(right)).into_split();
        let sealed: Vec<u8> = (0..50_000).map(|index| (index % 251) as u8).collect();

        let (send_result, received) = tokio::join!(sender.send(Tag::Video, &sealed), receiver.recv());
        send_result.unwrap();
        assert_eq!(received, Some((Tag::Video, sealed)));
    }

    #[test]
    fn reversed_fragments_reassemble() {
        let message = frame::wrap(Tag::Data, &[7u8; 5_000]);
        let mut fragments = fragment_message(&message, 42).unwrap();
        fragments.reverse();
        let mut reassembler = Reassembler::new(MAX_IN_FLIGHT_MESSAGES);
        let mut result = None;
        for fragment in fragments {
            result = reassembler.push(&fragment).or(result);
        }
        assert_eq!(result, Some(message));
    }

    #[test]
    fn evicts_oldest_incomplete_message_at_bound() {
        let mut reassembler = Reassembler::new(2);
        for msg_id in 0..3 {
            let mut message = vec![msg_id as u8; 1_101];
            message[0] = Tag::Video as u8;
            let fragments = fragment_message(&message, msg_id);
            let first_fragment = fragments.unwrap().into_iter().next().unwrap();
            assert!(reassembler.push(&first_fragment).is_none());
        }
        assert_eq!(reassembler.messages.len(), 2);
        assert!(!reassembler.messages.contains_key(&0));
        assert!(reassembler.messages.contains_key(&1));
        assert!(reassembler.messages.contains_key(&2));
    }
}
