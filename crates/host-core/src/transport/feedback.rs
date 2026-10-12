//! Sealed transport feedback packets on the direct UDP path.
//!
//! A feedback datagram shares the 8-byte fragment header layout but sets
//! the fragment count to 0, which every fragment reassembler (including
//! beta.4 and older) rejects as malformed and drops. Old peers therefore
//! ignore feedback, and the features built on it (retransmission, bitrate
//! adaptation) stay off with them.
//!
//! ```text
//! [magic u32 "ODFB"][version u16 = 1][0u16] [12-byte nonce][ChaCha20-Poly1305 ciphertext+tag]
//! ```
//!
//! The 8-byte header is the AEAD associated data. Each direction has its own
//! key (`protocol::kex::derive_transport_keys`) and its own nonce counter,
//! and the opener's replay window rejects replays. Plaintext is a type byte
//! and a fixed big-endian body; unknown types are ignored, so new feedback
//! kinds can be added without breaking peers.

use protocol::{Opener, Sealer};

pub(crate) const HEADER_LEN: usize = 8;
const MAGIC: [u8; 4] = *b"ODFB";
const VERSION: [u8; 2] = 1u16.to_be_bytes();
const HEADER: [u8; HEADER_LEN] = [MAGIC[0], MAGIC[1], MAGIC[2], MAGIC[3], VERSION[0], VERSION[1], 0, 0];

const SENDER_REPORT: u8 = 1;
const RECEIVER_REPORT: u8 = 2;
const NACK: u8 = 3;

/// Ranges per NACK packet: keeps the datagram near the media fragment size.
pub(crate) const MAX_NACK_RANGES: usize = 128;
/// A NACK range of this length asks for every fragment of the message
/// (used when no fragment of it arrived, so its size is unknown).
pub(crate) const WHOLE_MESSAGE: u16 = u16::MAX;

/// Missing fragments `start .. start + count` of message `msg_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NackRange {
    pub msg_id: u32,
    pub start: u16,
    pub count: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Feedback {
    /// Media sender → receiver, periodically. Announces that the sender
    /// retransmits, carries a timestamp for RTT measurement and the
    /// sender's smoothed RTT (µs, 0 = unknown) for the receiver's NACK timers.
    SenderReport { send_time_us: u64, rtt_us: u32 },
    /// Receiver → sender, in reply to each sender report. Counters are
    /// cumulative and wrap; the sender works on differences.
    ReceiverReport {
        echo_send_time_us: u64,
        /// Time between receiving the sender report and sending this.
        hold_us: u32,
        /// Unique fragments received (first transmissions and retransmissions).
        received: u32,
        /// Fragments found missing (each counted once).
        lost: u32,
        /// Missing fragments that later arrived.
        recovered: u32,
        /// Messages given up on.
        abandoned: u32,
    },
    /// Receiver → sender: please resend these fragments.
    Nack(Vec<NackRange>),
}

impl Feedback {
    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            Self::SenderReport { send_time_us, rtt_us } => {
                out.push(SENDER_REPORT);
                out.extend_from_slice(&send_time_us.to_be_bytes());
                out.extend_from_slice(&rtt_us.to_be_bytes());
            }
            Self::ReceiverReport { echo_send_time_us, hold_us, received, lost, recovered, abandoned } => {
                out.push(RECEIVER_REPORT);
                out.extend_from_slice(&echo_send_time_us.to_be_bytes());
                for value in [hold_us, received, lost, recovered, abandoned] {
                    out.extend_from_slice(&value.to_be_bytes());
                }
            }
            Self::Nack(ranges) => {
                out.push(NACK);
                let ranges = &ranges[..ranges.len().min(MAX_NACK_RANGES)];
                out.extend_from_slice(&(ranges.len() as u16).to_be_bytes());
                for range in ranges {
                    out.extend_from_slice(&range.msg_id.to_be_bytes());
                    out.extend_from_slice(&range.start.to_be_bytes());
                    out.extend_from_slice(&range.count.to_be_bytes());
                }
            }
        }
        out
    }

    fn decode(plain: &[u8]) -> Option<Self> {
        let (&kind, body) = plain.split_first()?;
        let u16_at = |at: usize| Some(u16::from_be_bytes(body.get(at..at + 2)?.try_into().ok()?));
        let u32_at = |at: usize| Some(u32::from_be_bytes(body.get(at..at + 4)?.try_into().ok()?));
        let u64_at = |at: usize| Some(u64::from_be_bytes(body.get(at..at + 8)?.try_into().ok()?));
        match kind {
            SENDER_REPORT => Some(Self::SenderReport { send_time_us: u64_at(0)?, rtt_us: u32_at(8)? }),
            RECEIVER_REPORT => Some(Self::ReceiverReport {
                echo_send_time_us: u64_at(0)?,
                hold_us: u32_at(8)?,
                received: u32_at(12)?,
                lost: u32_at(16)?,
                recovered: u32_at(20)?,
                abandoned: u32_at(24)?,
            }),
            NACK => {
                let count = usize::from(u16_at(0)?);
                if count > MAX_NACK_RANGES {
                    return None;
                }
                let ranges = (0..count)
                    .map(|index| {
                        let at = 2 + index * 8;
                        Some(NackRange { msg_id: u32_at(at)?, start: u16_at(at + 4)?, count: u16_at(at + 6)? })
                    })
                    .collect::<Option<Vec<_>>>()?;
                Some(Self::Nack(ranges))
            }
            _ => None,
        }
    }

    /// Seals into a datagram. Each call consumes a fresh nonce.
    pub(crate) fn seal(&self, sealer: &mut Sealer) -> Vec<u8> {
        let sealed = sealer.seal(&HEADER, &self.encode());
        let mut datagram = Vec::with_capacity(HEADER_LEN + sealed.len());
        datagram.extend_from_slice(&HEADER);
        datagram.extend_from_slice(&sealed);
        datagram
    }

    /// Authenticates and decodes a feedback datagram; `None` for anything
    /// forged, replayed, of another version or of an unknown type.
    pub(crate) fn open(opener: &mut Opener, datagram: &[u8]) -> Option<Self> {
        if datagram.get(..HEADER_LEN)? != HEADER {
            return None;
        }
        Self::decode(&opener.open(&HEADER, &datagram[HEADER_LEN..])?)
    }
}

/// Whether a datagram is a feedback packet rather than a media fragment
/// (its fragment count field is 0, which no fragment uses).
pub(crate) fn is_feedback(datagram: &[u8]) -> bool {
    datagram.len() >= HEADER_LEN && datagram[6..8] == [0, 0]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair() -> (Sealer, Opener) { (Sealer::new([3; 32]), Opener::new([3; 32])) }

    fn samples() -> Vec<Feedback> {
        vec![
            Feedback::SenderReport { send_time_us: 0x0102_0304_0506_0708, rtt_us: 25_000 },
            Feedback::ReceiverReport { echo_send_time_us: 77, hold_us: 3, received: 1_000, lost: 12, recovered: 10, abandoned: 1 },
            Feedback::Nack(vec![
                NackRange { msg_id: u32::MAX, start: 0, count: WHOLE_MESSAGE },
                NackRange { msg_id: 5, start: 17, count: 3 },
            ]),
            Feedback::Nack(Vec::new()),
        ]
    }

    #[test]
    fn every_kind_round_trips_sealed() {
        let (mut sealer, mut opener) = pair();
        for feedback in samples() {
            let datagram = feedback.seal(&mut sealer);
            assert!(is_feedback(&datagram));
            assert!(datagram.len() <= HEADER_LEN + 1_100);
            assert_eq!(Feedback::open(&mut opener, &datagram), Some(feedback));
        }
    }

    #[test]
    fn a_full_nack_still_fits_a_fragment_sized_datagram() {
        let (mut sealer, mut opener) = pair();
        let ranges: Vec<_> = (0..MAX_NACK_RANGES as u32 + 10).map(|id| NackRange { msg_id: id, start: 0, count: 1 }).collect();
        let datagram = Feedback::Nack(ranges.clone()).seal(&mut sealer);
        assert!(datagram.len() <= HEADER_LEN + 1_100, "{}", datagram.len());
        assert_eq!(Feedback::open(&mut opener, &datagram), Some(Feedback::Nack(ranges[..MAX_NACK_RANGES].to_vec())));
    }

    #[test]
    fn rejects_tampering_replay_wrong_key_and_unknown_types() {
        let (mut sealer, mut opener) = pair();
        let datagram = samples()[0].seal(&mut sealer);
        let mut tampered = datagram.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert_eq!(Feedback::open(&mut opener, &tampered), None);
        let mut bad_header = datagram.clone();
        bad_header[0] ^= 1;
        assert_eq!(Feedback::open(&mut opener, &bad_header), None);
        assert!(Feedback::open(&mut opener, &datagram).is_some());
        assert_eq!(Feedback::open(&mut opener, &datagram), None, "replay");
        assert_eq!(Feedback::open(&mut Opener::new([4; 32]), &samples()[1].seal(&mut sealer)), None);
        let unknown = { let mut s = Sealer::new([3; 32]); let mut d = HEADER.to_vec(); d.extend(s.seal(&HEADER, &[0xEE, 1, 2])); d };
        assert_eq!(Feedback::open(&mut Opener::new([3; 32]), &unknown), None);
    }

    #[test]
    fn nonces_never_repeat() {
        let (mut sealer, _) = pair();
        let feedback = Feedback::Nack(Vec::new());
        let nonces: std::collections::HashSet<Vec<u8>> =
            (0..100).map(|_| feedback.seal(&mut sealer)[HEADER_LEN..HEADER_LEN + 12].to_vec()).collect();
        assert_eq!(nonces.len(), 100);
    }

    #[test]
    fn feedback_never_looks_like_a_fragment() {
        let (mut sealer, _) = pair();
        for feedback in samples() {
            let datagram = feedback.seal(&mut sealer);
            // Old reassemblers drop any datagram whose fragment count is 0.
            assert_eq!(u16::from_be_bytes([datagram[6], datagram[7]]), 0);
        }
        assert!(!is_feedback(&[0, 0, 0, 1, 0, 0, 0, 1, 9]));
        assert!(!is_feedback(&[0; 7]));
    }
}
