//! UDP fragment format, receive-side reassembly with loss recovery, and the
//! sender's retransmission ring.
//!
//! Every wrapped frame is split into datagrams carrying an 8-byte header
//! (network byte order): message id `u32`, fragment index `u16`, fragment
//! count `u16`. Message ids increase by one per message in the order their
//! first fragment is sent.
//!
//! The receiver starts in legacy mode: messages are delivered as soon as
//! they are complete and incomplete ones are evicted (what beta.4 does).
//! Once the sender proves it retransmits (an authenticated sender report),
//! the receiver delivers messages in id order, asks for missing fragments
//! (NACK) and gives a message up only after a few unanswered retries, so
//! the decoder sees one in-order stream instead of a gap per lost datagram.

use super::feedback::{NackRange, MAX_NACK_RANGES, WHOLE_MESSAGE};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(crate) const FRAG_HEADER_LEN: usize = 8;
/// Keep UDP datagrams comfortably below the usual Internet path MTU.
pub(crate) const FRAG_PAYLOAD: usize = 1100;
/// Larger fragment counts are rejected: 8192 fragments is ~9 MB, more than
/// twice the largest frame the relay accepts.
const MAX_FRAG_COUNT: u16 = 8192;
/// The largest wrapped frame that can be fragmented.
pub(crate) const MAX_MESSAGE_LEN: usize = MAX_FRAG_COUNT as usize * FRAG_PAYLOAD;
/// Incomplete messages kept in legacy mode.
const LEGACY_MAX_IN_FLIGHT: usize = 8;

/// Messages between the oldest undelivered one and the newest seen.
const MAX_WINDOW: u32 = 256;
/// Bytes (payload plus bookkeeping) held for undelivered messages.
const MAX_HELD_BYTES: usize = 32 * 1024 * 1024;
/// NACKs per message before it is given up on.
pub(crate) const MAX_NACKS: u8 = 3;
/// A gap must persist this long before it is NACKed (reordering).
const REORDER_GRACE: Duration = Duration::from_millis(3);
/// A message with no new fragment for this long is missing its tail.
const TAIL_TIMEOUT: Duration = Duration::from_millis(15);
/// A message with no progress for this long is given up on regardless.
const STALL_TIMEOUT: Duration = Duration::from_millis(500);
const DEFAULT_RTT: Duration = Duration::from_millis(50);

/// Time between NACKs for one message: about one RTT.
pub(crate) fn retry_interval(rtt: Duration) -> Duration {
    (rtt + rtt / 4 + Duration::from_millis(5)).clamp(Duration::from_millis(15), Duration::from_millis(150))
}

/// One parsed media fragment.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Fragment<'a> {
    pub msg_id: u32,
    pub index: u16,
    pub count: u16,
    pub payload: &'a [u8],
}

pub(crate) fn parse_fragment(datagram: &[u8]) -> Option<Fragment<'_>> {
    if datagram.len() < FRAG_HEADER_LEN || datagram.len() > FRAG_HEADER_LEN + FRAG_PAYLOAD {
        return None;
    }
    let msg_id = u32::from_be_bytes(datagram[0..4].try_into().ok()?);
    let index = u16::from_be_bytes(datagram[4..6].try_into().ok()?);
    let count = u16::from_be_bytes(datagram[6..8].try_into().ok()?);
    if count == 0 || index >= count || count > MAX_FRAG_COUNT {
        return None;
    }
    Some(Fragment { msg_id, index, count, payload: &datagram[FRAG_HEADER_LEN..] })
}

/// Fragments a complete wrapped frame.
pub(crate) fn fragment_message(message: &[u8], msg_id: u32) -> Result<Vec<Arc<[u8]>>, &'static str> {
    if message.is_empty() {
        return Err("cannot fragment an empty message");
    }
    let frag_count = message.len().div_ceil(FRAG_PAYLOAD);
    let frag_count = u16::try_from(frag_count).ok().filter(|&count| count <= MAX_FRAG_COUNT).ok_or("message has too many fragments")?;
    Ok(message
        .chunks(FRAG_PAYLOAD)
        .enumerate()
        .map(|(index, chunk)| {
            let mut datagram = Vec::with_capacity(FRAG_HEADER_LEN + chunk.len());
            datagram.extend_from_slice(&msg_id.to_be_bytes());
            datagram.extend_from_slice(&(index as u16).to_be_bytes());
            datagram.extend_from_slice(&frag_count.to_be_bytes());
            datagram.extend_from_slice(chunk);
            Arc::from(datagram)
        })
        .collect())
}

/// Cumulative receive counters (wrapping), reported to the sender.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RecvStats {
    pub received: u32,
    pub lost: u32,
    pub recovered: u32,
    pub abandoned: u32,
}

/// NACK bookkeeping of one fragment (or of a wholly missing message).
#[derive(Debug, Clone, Copy, Default)]
struct NackState {
    count: u8,
    last: Option<Instant>,
}

impl NackState {
    fn due(self, now: Instant, retry: Duration) -> bool {
        self.count < MAX_NACKS && self.last.is_none_or(|at| now.duration_since(at) >= retry)
    }

    /// Retries are used up and the last one went unanswered.
    fn exhausted(self, now: Instant, retry: Duration) -> bool {
        self.count >= MAX_NACKS && self.last.is_some_and(|at| now.duration_since(at) >= retry)
    }

    /// Records a NACK; returns whether it was the first (a new loss).
    fn record(&mut self, now: Instant) -> bool {
        self.count += 1;
        self.last = Some(now);
        self.count == 1
    }
}

#[derive(Debug)]
struct Slot {
    /// 0 while no fragment arrived (the message is known only from a gap
    /// in message ids).
    frag_count: u16,
    fragments: Vec<Option<Box<[u8]>>>,
    /// Per fragment: each has its own retry budget.
    nacks: Vec<NackState>,
    /// NACKs for the whole message while its size is unknown.
    whole: NackState,
    received: u16,
    highest_index: u16,
    gap_since: Option<Instant>,
    /// Creation or the last new fragment.
    last_activity: Instant,
    /// Payload and bookkeeping bytes, for the memory bound.
    bytes: usize,
    complete: Option<Vec<u8>>,
}

impl Slot {
    fn missing(now: Instant) -> Self {
        Self {
            frag_count: 0,
            fragments: Vec::new(),
            nacks: Vec::new(),
            whole: NackState::default(),
            received: 0,
            highest_index: 0,
            gap_since: Some(now),
            last_activity: now,
            bytes: 64,
            complete: None,
        }
    }

    /// Stores a fragment. Returns `None` for a duplicate or inconsistent
    /// one, else the NACK state of a fragment that was asked for (it is a
    /// recovery).
    fn add(&mut self, now: Instant, fragment: Fragment<'_>) -> Option<Option<NackState>> {
        if self.complete.is_some() {
            return None;
        }
        let first = self.frag_count == 0;
        if first {
            let count = usize::from(fragment.count);
            self.frag_count = fragment.count;
            self.fragments = vec![None; count];
            self.nacks = vec![NackState::default(); count];
            self.bytes += count * (std::mem::size_of::<Option<Box<[u8]>>>() + std::mem::size_of::<NackState>());
            self.gap_since = None;
        } else if self.frag_count != fragment.count {
            return None;
        }
        let index = usize::from(fragment.index);
        if self.fragments[index].is_some() {
            return None;
        }
        self.fragments[index] = Some(fragment.payload.into());
        self.bytes += fragment.payload.len();
        self.received += 1;
        self.highest_index = self.highest_index.max(fragment.index);
        self.last_activity = now;
        let recovered = if first && self.whole.count > 0 {
            Some(self.whole)
        } else {
            Some(self.nacks[index]).filter(|state| state.count > 0)
        };
        if self.received == self.frag_count {
            let mut message = Vec::with_capacity(self.fragments.iter().flatten().map(|part| part.len()).sum());
            for part in self.fragments.drain(..).flatten() {
                message.extend_from_slice(&part);
            }
            self.complete = Some(message);
        } else if self.received <= self.highest_index {
            // Something below the highest fragment is missing.
            self.gap_since.get_or_insert(now);
        } else {
            self.gap_since = None;
        }
        Some(recovered)
    }

    /// Whether to give the message up: a missing fragment used up its
    /// retries, or nothing happened for a long time.
    fn expired(&self, now: Instant, retry: Duration) -> bool {
        if self.complete.is_some() {
            return false;
        }
        if now.duration_since(self.last_activity) >= STALL_TIMEOUT {
            return true;
        }
        if self.frag_count == 0 {
            return self.whole.exhausted(now, retry);
        }
        self.fragments.iter().zip(&self.nacks).any(|(fragment, state)| fragment.is_none() && state.exhausted(now, retry))
    }

    /// Appends the ranges to NACK now and counts newly lost fragments.
    fn nack(&mut self, now: Instant, retry: Duration, msg_id: u32, ranges: &mut Vec<NackRange>, lost: &mut u32) {
        if self.complete.is_some() {
            return;
        }
        if self.frag_count == 0 {
            let settled = self.gap_since.is_some_and(|at| now.duration_since(at) >= REORDER_GRACE);
            if settled && self.whole.due(now, retry) && ranges.len() < MAX_NACK_RANGES {
                ranges.push(NackRange { msg_id, start: 0, count: WHOLE_MESSAGE });
                if self.whole.record(now) {
                    *lost = lost.wrapping_add(1);
                }
            }
            return;
        }
        let limit = if now.duration_since(self.last_activity) >= TAIL_TIMEOUT {
            // Nothing new for a while: the tail is missing too.
            self.frag_count
        } else if self.gap_since.is_some_and(|at| now.duration_since(at) >= REORDER_GRACE) {
            self.highest_index
        } else {
            return;
        };
        let wanted = |slot: &Self, index: u16| {
            slot.fragments[usize::from(index)].is_none() && slot.nacks[usize::from(index)].due(now, retry)
        };
        let mut index = 0u16;
        while index < limit && ranges.len() < MAX_NACK_RANGES {
            if !wanted(self, index) {
                index += 1;
                continue;
            }
            let start = index;
            while index < limit && wanted(self, index) {
                if self.nacks[usize::from(index)].record(now) {
                    *lost = lost.wrapping_add(1);
                }
                index += 1;
            }
            ranges.push(NackRange { msg_id, start, count: index - start });
        }
    }
}

/// Receive side of the fragment layer (see the module docs).
#[derive(Debug)]
pub(crate) struct ReceiveTracker {
    ordered: bool,
    slots: HashMap<u32, Slot>,
    /// Legacy mode: eviction order.
    legacy_order: VecDeque<u32>,
    /// Ordered mode: every id in `next..end` has a slot.
    next: Option<u32>,
    end: u32,
    held_bytes: usize,
    ready: VecDeque<Vec<u8>>,
    stats: RecvStats,
    rtt: Duration,
    rtt_from_sender: bool,
}

impl Default for ReceiveTracker {
    fn default() -> Self { Self::new() }
}

impl ReceiveTracker {
    pub(crate) fn new() -> Self {
        Self {
            ordered: false,
            slots: HashMap::new(),
            legacy_order: VecDeque::new(),
            next: None,
            end: 0,
            held_bytes: 0,
            ready: VecDeque::new(),
            stats: RecvStats::default(),
            rtt: DEFAULT_RTT,
            rtt_from_sender: false,
        }
    }

    pub(crate) fn is_ordered(&self) -> bool { self.ordered }

    /// The sender retransmits: switch to ordered delivery with NACKs.
    pub(crate) fn enable_ordered(&mut self) {
        if self.ordered {
            return;
        }
        self.ordered = true;
        self.slots.clear();
        self.legacy_order.clear();
        self.held_bytes = 0;
    }

    /// The sender's RTT measurement (preferred over our own samples).
    pub(crate) fn set_rtt(&mut self, rtt: Duration) {
        self.rtt = rtt;
        self.rtt_from_sender = true;
    }

    pub(crate) fn stats(&self) -> RecvStats { self.stats }

    /// The next message ready for delivery, in order.
    pub(crate) fn pop_ready(&mut self) -> Option<Vec<u8>> { self.ready.pop_front() }

    pub(crate) fn push(&mut self, now: Instant, datagram: &[u8]) {
        let Some(fragment) = parse_fragment(datagram) else { return };
        if self.ordered { self.push_ordered(now, fragment) } else { self.push_legacy(now, fragment) }
    }

    fn push_legacy(&mut self, now: Instant, fragment: Fragment<'_>) {
        if !self.slots.contains_key(&fragment.msg_id) {
            if self.slots.len() >= LEGACY_MAX_IN_FLIGHT {
                while let Some(oldest) = self.legacy_order.pop_front() {
                    if self.slots.remove(&oldest).is_some() {
                        break;
                    }
                }
            }
            self.slots.insert(fragment.msg_id, Slot::missing(now));
            self.legacy_order.push_back(fragment.msg_id);
        }
        let Some(slot) = self.slots.get_mut(&fragment.msg_id) else { return };
        if slot.add(now, fragment).is_none() {
            return;
        }
        self.stats.received = self.stats.received.wrapping_add(1);
        if let Some(message) = slot.complete.take() {
            self.slots.remove(&fragment.msg_id);
            self.legacy_order.retain(|&id| id != fragment.msg_id);
            self.ready.push_back(message);
        }
    }

    fn push_ordered(&mut self, now: Instant, fragment: Fragment<'_>) {
        let msg_id = fragment.msg_id;
        let next = *self.next.get_or_insert_with(|| {
            self.end = msg_id;
            msg_id
        });
        let offset = msg_id.wrapping_sub(next);
        if offset >= 1 << 31 {
            return; // Already delivered or given up on.
        }
        if offset >= MAX_WINDOW {
            // Far ahead: everything older is hopeless. Deliver what is
            // complete and restart the window at this message.
            self.flush_until(msg_id);
        }
        while msg_id.wrapping_sub(self.end) < 1 << 31 {
            // msg_id >= end: open slots up to and including it.
            self.slots.insert(self.end, Slot::missing(now));
            self.held_bytes += 64;
            self.end = self.end.wrapping_add(1);
        }
        let Some(slot) = self.slots.get_mut(&msg_id) else { return };
        let before = slot.bytes;
        let Some(recovered) = slot.add(now, fragment) else { return };
        self.held_bytes += slot.bytes - before;
        self.stats.received = self.stats.received.wrapping_add(1);
        if let Some(nacked) = recovered {
            self.stats.recovered = self.stats.recovered.wrapping_add(1);
            // Karn: only unambiguous (single NACK) samples.
            if let (1, Some(at), false) = (nacked.count, nacked.last, self.rtt_from_sender) {
                self.rtt = (self.rtt * 3 + now.duration_since(at)) / 4;
            }
        }
        self.release();
        while self.held_bytes > MAX_HELD_BYTES && self.drop_head() {}
    }

    /// Delivers complete messages at the head of the window.
    fn release(&mut self) {
        while let Some(next) = self.next {
            if next == self.end || !self.slots.get(&next).is_some_and(|slot| slot.complete.is_some()) {
                break;
            }
            let mut slot = self.slots.remove(&next).expect("slot exists");
            self.held_bytes -= slot.bytes;
            self.ready.extend(slot.complete.take());
            self.next = Some(next.wrapping_add(1));
        }
    }

    /// Gives up on the head message. Returns false when the window is empty.
    fn drop_head(&mut self) -> bool {
        let Some(next) = self.next.filter(|&next| next != self.end) else { return false };
        if let Some(slot) = self.slots.remove(&next) {
            self.held_bytes -= slot.bytes;
        }
        self.stats.abandoned = self.stats.abandoned.wrapping_add(1);
        self.next = Some(next.wrapping_add(1));
        self.release();
        true
    }

    /// Delivers or abandons everything before `msg_id` and restarts the
    /// window there.
    fn flush_until(&mut self, msg_id: u32) {
        while self.next.is_some_and(|next| next != self.end) {
            self.release();
            if !self.drop_head() {
                break;
            }
        }
        self.slots.clear();
        self.held_bytes = 0;
        self.next = Some(msg_id);
        self.end = msg_id;
    }

    /// Advances timers: gives up on expired messages at the head and
    /// returns the fragments to NACK now (at most one packet's worth).
    pub(crate) fn poll(&mut self, now: Instant) -> Vec<NackRange> {
        let mut ranges = Vec::new();
        if !self.ordered {
            return ranges;
        }
        let retry = retry_interval(self.rtt);
        while let Some(next) = self.next.filter(|&next| next != self.end) {
            if !self.slots.get(&next).is_some_and(|slot| slot.expired(now, retry)) {
                break;
            }
            self.drop_head();
        }
        let Some(mut id) = self.next else { return ranges };
        while id != self.end && ranges.len() < MAX_NACK_RANGES {
            if let Some(slot) = self.slots.get_mut(&id) {
                slot.nack(now, retry, id, &mut ranges, &mut self.stats.lost);
            }
            id = id.wrapping_add(1);
        }
        ranges
    }
}

/// Recently sent messages, kept for retransmission.
#[derive(Debug, Default)]
pub(crate) struct RetransmitRing {
    /// Consecutive message ids, oldest first.
    entries: VecDeque<RingEntry>,
    bytes: usize,
}

#[derive(Debug)]
struct RingEntry {
    msg_id: u32,
    fragments: Vec<Arc<[u8]>>,
    /// Fragments go out in order; only sent ones can be resent.
    sent: u16,
    resends: Vec<u8>,
}

const RING_MAX_MESSAGES: usize = 256;
const RING_MAX_BYTES: usize = 4 * 1024 * 1024;
/// Retransmissions per fragment; bounds what a NACK flood can trigger.
const MAX_RESENDS: u8 = MAX_NACKS;

impl RetransmitRing {
    /// Records a message whose first fragment is about to be sent. Ids
    /// must be consecutive.
    pub(crate) fn insert(&mut self, msg_id: u32, fragments: Vec<Arc<[u8]>>) {
        if self.entries.back().is_some_and(|last| last.msg_id.wrapping_add(1) != msg_id) {
            self.entries.clear();
            self.bytes = 0;
        }
        self.bytes += fragments.iter().map(|fragment| fragment.len()).sum::<usize>();
        let resends = vec![0; fragments.len()];
        self.entries.push_back(RingEntry { msg_id, fragments, sent: 0, resends });
        while self.entries.len() > RING_MAX_MESSAGES || (self.bytes > RING_MAX_BYTES && self.entries.len() > 1) {
            let evicted = self.entries.pop_front().expect("ring is not empty");
            self.bytes -= evicted.fragments.iter().map(|fragment| fragment.len()).sum::<usize>();
        }
    }

    fn entry(&mut self, msg_id: u32) -> Option<&mut RingEntry> {
        let first = self.entries.front()?.msg_id;
        self.entries.get_mut(usize::try_from(msg_id.wrapping_sub(first)).ok()?)
    }

    pub(crate) fn mark_sent(&mut self, msg_id: u32, index: u16) {
        if let Some(entry) = self.entry(msg_id) {
            entry.sent = entry.sent.max(index + 1);
        }
    }

    /// Appends the already-sent fragments in `range` that may still be
    /// resent, stopping at `limit` datagrams in `out`.
    pub(crate) fn resend(&mut self, range: NackRange, out: &mut Vec<Arc<[u8]>>, limit: usize) {
        let Some(entry) = self.entry(range.msg_id) else { return };
        let end = if range.count == WHOLE_MESSAGE { entry.sent } else { range.start.saturating_add(range.count).min(entry.sent) };
        for index in usize::from(range.start)..usize::from(end) {
            if out.len() >= limit {
                return;
            }
            if entry.resends[index] < MAX_RESENDS {
                entry.resends[index] += 1;
                out.push(Arc::clone(&entry.fragments[index]));
            }
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize { self.entries.len() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{frame, Tag};

    fn message(len: usize, seed: u8) -> Vec<u8> {
        let mut message = frame::wrap(Tag::Video, &vec![seed; len]);
        message[1] = seed;
        message
    }

    fn fragments(message: &[u8], msg_id: u32) -> Vec<Arc<[u8]>> { fragment_message(message, msg_id).unwrap() }

    fn ordered() -> ReceiveTracker {
        let mut tracker = ReceiveTracker::new();
        tracker.enable_ordered();
        tracker
    }

    fn drain(tracker: &mut ReceiveTracker) -> Vec<Vec<u8>> { std::iter::from_fn(|| tracker.pop_ready()).collect() }

    #[test]
    fn reversed_fragments_reassemble() {
        let message = frame::wrap(Tag::Data, &[7u8; 5_000]);
        for mut tracker in [ReceiveTracker::new(), ordered()] {
            let now = Instant::now();
            for fragment in fragments(&message, 42).iter().rev() {
                tracker.push(now, fragment);
            }
            assert_eq!(drain(&mut tracker), vec![message.clone()]);
        }
    }

    #[test]
    fn legacy_mode_evicts_the_oldest_incomplete_message_at_its_bound() {
        let mut tracker = ReceiveTracker::new();
        let now = Instant::now();
        for msg_id in 0..LEGACY_MAX_IN_FLIGHT as u32 + 1 {
            tracker.push(now, &fragments(&message(1_500, 1), msg_id)[0]);
        }
        assert_eq!(tracker.slots.len(), LEGACY_MAX_IN_FLIGHT);
        assert!(!tracker.slots.contains_key(&0));
        // Legacy mode delivers out of order and never NACKs.
        tracker.push(now, &fragments(&message(10, 2), 100)[0]);
        assert_eq!(drain(&mut tracker), vec![message(10, 2)]);
        assert!(tracker.poll(now + Duration::from_secs(1)).is_empty());
    }

    #[test]
    fn rejects_malformed_fragments_and_feedback_packets() {
        let now = Instant::now();
        let mut tracker = ReceiveTracker::new();
        tracker.push(now, &[0, 0, 0, 1, 0, 0, 0, 0, 9]); // count 0 (feedback)
        tracker.push(now, &[0, 0, 0, 1, 0, 2, 0, 2, 9]); // index >= count
        tracker.push(now, &[0, 0, 0, 1]);
        tracker.push(now, &vec![0; FRAG_HEADER_LEN + FRAG_PAYLOAD + 1]);
        assert!(tracker.slots.is_empty());
        assert!(fragment_message(&[], 0).is_err());
    }

    #[test]
    fn ordered_mode_holds_later_messages_until_a_gap_is_filled() {
        let mut tracker = ordered();
        let now = Instant::now();
        let first = message(3_000, 1);
        let second = message(500, 2);
        let first_fragments = fragments(&first, 10);
        tracker.push(now, &first_fragments[0]);
        tracker.push(now, &first_fragments[2]);
        tracker.push(now, &fragments(&second, 11)[0]);
        assert!(drain(&mut tracker).is_empty(), "message 11 waits for 10");
        tracker.push(now, &first_fragments[1]);
        assert_eq!(drain(&mut tracker), vec![first, second]);
        // Late duplicates of delivered messages are ignored.
        tracker.push(now, &first_fragments[1]);
        assert!(drain(&mut tracker).is_empty());
        assert!(tracker.slots.is_empty());
    }

    #[test]
    fn nacks_inner_gaps_after_the_reorder_grace() {
        let mut tracker = ordered();
        let start = Instant::now();
        let parts = fragments(&message(6_000, 1), 0);
        for index in [0, 1, 4] {
            tracker.push(start, &parts[index]);
        }
        assert!(tracker.poll(start).is_empty(), "within the reorder grace");
        let ranges = tracker.poll(start + REORDER_GRACE);
        assert_eq!(ranges, vec![NackRange { msg_id: 0, start: 2, count: 2 }]);
        assert_eq!(tracker.stats().lost, 2);
        // Not again before the retry interval.
        assert!(tracker.poll(start + REORDER_GRACE + Duration::from_millis(1)).is_empty());
    }

    #[test]
    fn nacks_a_missing_tail_after_the_tail_timeout() {
        let mut tracker = ordered();
        let start = Instant::now();
        let parts = fragments(&message(6_000, 1), 0);
        tracker.push(start, &parts[0]);
        tracker.push(start, &parts[1]);
        assert!(tracker.poll(start + Duration::from_millis(5)).is_empty());
        let ranges = tracker.poll(start + TAIL_TIMEOUT);
        assert_eq!(ranges, vec![NackRange { msg_id: 0, start: 2, count: parts.len() as u16 - 2 }]);
    }

    #[test]
    fn nacks_wholly_missing_messages() {
        let mut tracker = ordered();
        let start = Instant::now();
        tracker.push(start, &fragments(&message(10, 1), 5)[0]);
        tracker.push(start, &fragments(&message(10, 3), 7)[0]);
        let ranges = tracker.poll(start + REORDER_GRACE);
        assert_eq!(ranges, vec![NackRange { msg_id: 6, start: 0, count: WHOLE_MESSAGE }]);
        assert_eq!(drain(&mut tracker), vec![message(10, 1)]);
        tracker.push(start, &fragments(&message(10, 2), 6)[0]);
        assert_eq!(drain(&mut tracker), vec![message(10, 2), message(10, 3)]);
        assert_eq!(tracker.stats(), RecvStats { received: 3, lost: 1, recovered: 1, abandoned: 0 });
    }

    #[test]
    fn retries_are_capped_then_the_message_is_abandoned() {
        let mut tracker = ordered();
        let mut now = Instant::now();
        let retry = retry_interval(DEFAULT_RTT);
        let lost = fragments(&message(3_000, 1), 0);
        tracker.push(now, &lost[0]);
        tracker.push(now, &lost[2]);
        tracker.push(now, &fragments(&message(10, 2), 1)[0]);
        now += REORDER_GRACE;
        for _ in 0..MAX_NACKS {
            assert_eq!(tracker.poll(now), vec![NackRange { msg_id: 0, start: 1, count: 1 }]);
            assert!(drain(&mut tracker).is_empty(), "message 1 waits while 0 is retried");
            now += retry - Duration::from_millis(1);
            assert!(tracker.poll(now).is_empty(), "no NACK before the retry interval");
            now += Duration::from_millis(1);
        }
        // Retries are used up: message 0 is given up on, message 1 goes out.
        assert!(tracker.poll(now).is_empty());
        assert_eq!(drain(&mut tracker), vec![message(10, 2)]);
        assert_eq!(tracker.stats().abandoned, 1);
        assert_eq!(tracker.stats().lost, 1, "a fragment counts as lost once");
    }

    #[test]
    fn each_fragment_has_its_own_retry_budget() {
        let mut tracker = ordered();
        let retry = retry_interval(DEFAULT_RTT);
        let start = Instant::now();
        let parts = fragments(&message(11_000, 1), 0);
        tracker.push(start, &parts[0]);
        tracker.push(start, &parts[2]);
        let mut now = start + REORDER_GRACE;
        assert_eq!(tracker.poll(now), vec![NackRange { msg_id: 0, start: 1, count: 1 }]);
        // A later loss in the same message is NACKed at once, without
        // waiting for (or spending) fragment 1's retries.
        for part in &parts[3..6] {
            tracker.push(now, part);
        }
        for part in &parts[7..] {
            tracker.push(now, part);
        }
        now += REORDER_GRACE;
        assert_eq!(tracker.poll(now), vec![NackRange { msg_id: 0, start: 6, count: 1 }]);
        for _ in 1..MAX_NACKS {
            now += retry;
            assert_eq!(
                tracker.poll(now),
                vec![NackRange { msg_id: 0, start: 1, count: 1 }, NackRange { msg_id: 0, start: 6, count: 1 }]
            );
        }
        // Fragment 6 arrives; fragment 1 is hopeless once its last retry
        // goes unanswered.
        tracker.push(now, &parts[6]);
        now += retry;
        assert!(tracker.poll(now).is_empty());
        assert_eq!(tracker.stats().abandoned, 1);
        assert_eq!(tracker.stats().lost, 2);
    }

    #[test]
    fn abandoned_message_is_skipped_and_its_late_fragments_ignored() {
        let mut tracker = ordered();
        let start = Instant::now();
        let parts = fragments(&message(3_000, 1), 0);
        tracker.push(start, &parts[0]);
        tracker.push(start, &fragments(&message(10, 2), 1)[0]);
        tracker.poll(start + STALL_TIMEOUT);
        assert_eq!(drain(&mut tracker), vec![message(10, 2)]);
        tracker.push(start + STALL_TIMEOUT, &parts[1]);
        tracker.push(start + STALL_TIMEOUT, &parts[2]);
        assert!(drain(&mut tracker).is_empty());
    }

    #[test]
    fn recovered_fragments_complete_the_message_and_sample_rtt() {
        let mut tracker = ordered();
        let start = Instant::now();
        let original = message(3_000, 9);
        let parts = fragments(&original, 3);
        tracker.push(start, &parts[0]);
        tracker.push(start, &parts[2]);
        assert_eq!(tracker.poll(start + REORDER_GRACE).len(), 1);
        tracker.push(start + REORDER_GRACE + Duration::from_millis(30), &parts[1]);
        assert_eq!(drain(&mut tracker), vec![original]);
        assert_eq!(tracker.stats().recovered, 1);
        assert_eq!(tracker.rtt, (DEFAULT_RTT * 3 + Duration::from_millis(30)) / 4);
    }

    #[test]
    fn a_far_jump_restarts_the_window() {
        let mut tracker = ordered();
        let now = Instant::now();
        tracker.push(now, &fragments(&message(3_000, 1), 0)[0]);
        tracker.push(now, &fragments(&message(10, 2), 1)[0]);
        tracker.push(now, &fragments(&message(10, 3), 10_000)[0]);
        assert_eq!(drain(&mut tracker), vec![message(10, 2), message(10, 3)]);
        assert_eq!(tracker.stats().abandoned, 1);
        assert!(tracker.slots.is_empty());
    }

    #[test]
    fn message_ids_wrap() {
        let mut tracker = ordered();
        let now = Instant::now();
        for (offset, msg_id) in [u32::MAX - 1, u32::MAX, 0, 1].into_iter().enumerate() {
            tracker.push(now, &fragments(&message(10, offset as u8), msg_id)[0]);
        }
        assert_eq!(drain(&mut tracker).len(), 4);
    }

    #[test]
    fn ring_resends_only_sent_fragments_and_caps_resends() {
        let mut ring = RetransmitRing::default();
        let parts = fragments(&message(5_000, 1), 7);
        ring.insert(7, parts.clone());
        for index in 0..3 {
            ring.mark_sent(7, index);
        }
        let mut out = Vec::new();
        ring.resend(NackRange { msg_id: 7, start: 0, count: WHOLE_MESSAGE }, &mut out, 100);
        assert_eq!(out, parts[..3].to_vec(), "unsent fragments are not resent");
        for _ in 0..10 {
            ring.resend(NackRange { msg_id: 7, start: 1, count: 1 }, &mut out, 100);
        }
        assert_eq!(out.len(), 3 + usize::from(MAX_RESENDS) - 1);
        out.clear();
        ring.resend(NackRange { msg_id: 8, start: 0, count: 1 }, &mut out, 100);
        ring.resend(NackRange { msg_id: 6, start: 0, count: 1 }, &mut out, 100);
        assert!(out.is_empty());
    }

    #[test]
    fn ring_evicts_oldest_messages_by_count_and_bytes() {
        let mut ring = RetransmitRing::default();
        for msg_id in 0..RING_MAX_MESSAGES as u32 + 10 {
            ring.insert(msg_id, fragments(&message(10, 1), msg_id));
        }
        assert_eq!(ring.len(), RING_MAX_MESSAGES);
        let mut out = Vec::new();
        ring.mark_sent(0, 0);
        ring.resend(NackRange { msg_id: 0, start: 0, count: 1 }, &mut out, 10);
        assert!(out.is_empty(), "evicted");

        let mut ring = RetransmitRing::default();
        let big = message(1_000_000, 1);
        for msg_id in 0..10 {
            ring.insert(msg_id, fragments(&big, msg_id));
        }
        assert!(ring.bytes <= RING_MAX_BYTES);
        assert_eq!(ring.len(), RING_MAX_BYTES / big.len());
        assert_eq!(ring.entries.front().unwrap().msg_id, 10 - ring.len() as u32);
    }

    #[test]
    fn ring_resets_on_non_consecutive_ids() {
        let mut ring = RetransmitRing::default();
        ring.insert(1, fragments(&message(10, 1), 1));
        ring.insert(5, fragments(&message(10, 1), 5));
        assert_eq!(ring.len(), 1);
    }
}
