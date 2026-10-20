//! Finding other Truedesk devices on the local network.
//!
//! While a device is discoverable it broadcasts a small announcement on a
//! fixed UDP port every few seconds; devices on the same network collect the
//! announcements into a registry that forgets a device shortly after its last
//! one. An announcement carries only the device's ID and its alias, so a
//! viewer can pick a device by name on the LAN instead of typing its ID.
//!
//! Only the ID and alias are broadcast, never the screen or any session data;
//! discovery is a convenience for the connect screen and can be turned off.

use crate::wol;
use std::collections::HashMap;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::Notify;

/// The UDP port devices announce themselves on. Arbitrary but fixed, and
/// unprivileged so no elevation is needed to bind it.
pub const DISCOVERY_PORT: u16 = 40111;
/// How often a discoverable device re-announces itself.
pub const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(3);
/// A device drops off the list this long after its last announcement.
pub const DEVICE_TTL: Duration = Duration::from_secs(10);

/// Marks our packets so unrelated UDP traffic on the port is ignored.
const MAGIC: [u8; 4] = *b"TDSC";
const VERSION: u8 = 1;
/// Longest alias we send or accept, in bytes. Keeps a packet well under an
/// Ethernet frame and bounds what a stranger on the LAN can make us store.
pub const MAX_ALIAS: usize = 64;
const MAX_ID: usize = 32;

/// What a device tells the network about itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Announce {
    pub id: String,
    pub alias: String,
}

/// Truncates `text` to at most `max` bytes without splitting a character.
fn clamp_utf8(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// `MAGIC | VERSION | id_len | id | alias_len | alias`, alias clamped to
/// [`MAX_ALIAS`]. IDs longer than [`MAX_ID`] are clamped too (they never are
/// in practice: an ID is nine digits).
pub fn encode_announce(id: &str, alias: &str) -> Vec<u8> {
    let id = clamp_utf8(id, MAX_ID).as_bytes();
    let alias = clamp_utf8(alias, MAX_ALIAS).as_bytes();
    let mut out = Vec::with_capacity(MAGIC.len() + 3 + id.len() + alias.len());
    out.extend_from_slice(&MAGIC);
    out.push(VERSION);
    out.push(id.len() as u8);
    out.extend_from_slice(id);
    out.push(alias.len() as u8);
    out.extend_from_slice(alias);
    out
}

/// Parses an announcement, returning `None` for anything that is not one or
/// is malformed. A valid announcement has a non-empty ID.
pub fn parse_announce(bytes: &[u8]) -> Option<Announce> {
    let rest = bytes.strip_prefix(&MAGIC)?;
    let (&version, rest) = rest.split_first()?;
    if version != VERSION {
        return None;
    }
    let (&id_len, rest) = rest.split_first()?;
    let (id, rest) = rest.split_at_checked(usize::from(id_len))?;
    let (&alias_len, rest) = rest.split_first()?;
    let (alias, _) = rest.split_at_checked(usize::from(alias_len))?;
    let id = std::str::from_utf8(id).ok()?;
    let alias = std::str::from_utf8(alias).ok()?;
    if id.is_empty() {
        return None;
    }
    Some(Announce { id: id.to_owned(), alias: alias.to_owned() })
}

/// A device seen on the network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub id: String,
    pub alias: String,
    pub addr: IpAddr,
}

/// The devices seen recently, each kept until [`Registry::ttl`] after its last
/// announcement.
pub struct Registry {
    ttl: Duration,
    seen: HashMap<String, (Device, Instant)>,
}

impl Registry {
    pub fn new(ttl: Duration) -> Self {
        Self { ttl, seen: HashMap::new() }
    }

    /// Records (or refreshes) a device from one announcement.
    pub fn observe(&mut self, announce: Announce, addr: IpAddr, now: Instant) {
        let device = Device { id: announce.id.clone(), alias: announce.alias, addr };
        self.seen.insert(announce.id, (device, now));
    }

    /// Forgets devices not heard from within the TTL.
    pub fn prune(&mut self, now: Instant) {
        let ttl = self.ttl;
        self.seen.retain(|_, (_, last)| now.duration_since(*last) < ttl);
    }

    /// The devices still live at `now`, by alias then ID so the list is stable.
    pub fn devices(&self, now: Instant) -> Vec<Device> {
        let mut devices: Vec<Device> = self
            .seen
            .values()
            .filter(|(_, last)| now.duration_since(*last) < self.ttl)
            .map(|(device, _)| device.clone())
            .collect();
        devices.sort_by(|a, b| a.alias.cmp(&b.alias).then_with(|| a.id.cmp(&b.id)));
        devices
    }
}

/// Announces this device and listens for others, keeping a live registry.
/// Dropping it, or calling [`Discovery::stop`], ends the future returned by
/// [`Discovery::start`].
pub struct Discovery {
    registry: Arc<Mutex<Registry>>,
    stop: Arc<Notify>,
    alias: Arc<Mutex<String>>,
}

impl Discovery {
    /// Prepares discovery for `id`/`alias` and returns the handle together
    /// with the future that does the work. The caller spawns the future on
    /// its own runtime (`tauri::async_runtime::spawn`); if the port can't be
    /// bound (another program has it), the future returns and the registry
    /// simply stays empty rather than failing.
    pub fn start(id: String, alias: String) -> (Self, impl Future<Output = ()> + Send) {
        let registry = Arc::new(Mutex::new(Registry::new(DEVICE_TTL)));
        let stop = Arc::new(Notify::new());
        let alias = Arc::new(Mutex::new(alias));
        let discovery = Self {
            registry: Arc::clone(&registry),
            stop: Arc::clone(&stop),
            alias: Arc::clone(&alias),
        };
        (discovery, run(id, alias, registry, stop))
    }

    /// The devices seen recently, freshest list (expired ones dropped).
    pub fn devices(&self) -> Vec<Device> {
        let mut registry = self.registry.lock().unwrap_or_else(|p| p.into_inner());
        let now = Instant::now();
        registry.prune(now);
        registry.devices(now)
    }

    /// Changes the name announced on the network; takes effect within a few
    /// seconds, on the next announcement.
    pub fn set_alias(&self, alias: String) {
        *self.alias.lock().unwrap_or_else(|p| p.into_inner()) = alias;
    }

    /// Stops announcing and listening; the work future returns soon after.
    pub fn stop(&self) {
        self.stop.notify_one();
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Broadcast addresses to announce to: each network's broadcast plus the
/// limited broadcast as a fallback, de-duplicated.
fn broadcast_addresses() -> Vec<Ipv4Addr> {
    let mut addresses: Vec<Ipv4Addr> = wol::local_targets()
        .into_iter()
        .filter_map(|target| target.broadcast.parse().ok())
        .collect();
    addresses.push(Ipv4Addr::BROADCAST);
    addresses.sort();
    addresses.dedup();
    addresses
}

/// Owns the socket and runs the announce and listen loops until stopped.
async fn run(id: String, alias: Arc<Mutex<String>>, registry: Arc<Mutex<Registry>>, stop: Arc<Notify>) {
    let socket = match UdpSocket::bind((Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT)).await {
        Ok(socket) => socket,
        Err(error) => {
            log::warn!("LAN discovery off: can't bind UDP {DISCOVERY_PORT}: {error}");
            return;
        }
    };
    if let Err(error) = socket.set_broadcast(true) {
        log::warn!("LAN discovery off: can't broadcast: {error}");
        return;
    }
    let socket = Arc::new(socket);
    let announcer = Arc::clone(&socket);
    let announce_id = id.clone();
    let announce = async move {
        loop {
            let packet = {
                let alias = alias.lock().unwrap_or_else(|p| p.into_inner());
                encode_announce(&announce_id, &alias)
            };
            for address in broadcast_addresses() {
                let _ = announcer.send_to(&packet, (address, DISCOVERY_PORT)).await;
            }
            tokio::time::sleep(ANNOUNCE_INTERVAL).await;
        }
    };

    let listen = async move {
        let mut buf = [0u8; 512];
        loop {
            let Ok((len, from)) = socket.recv_from(&mut buf).await else { continue };
            let Some(announce) = parse_announce(&buf[..len]) else { continue };
            // Skip our own announcements coming back on the broadcast.
            if announce.id == id {
                continue;
            }
            let now = Instant::now();
            let mut registry = registry.lock().unwrap_or_else(|p| p.into_inner());
            registry.observe(announce, from.ip(), now);
            registry.prune(now);
        }
    };

    // Either loop ending is unexpected; the stop signal is the normal exit.
    tokio::select! {
        _ = announce => {}
        _ = listen => {}
        _ = stop.notified() => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn addr() -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(192, 168, 1, 23))
    }

    #[test]
    fn announcements_round_trip() {
        let bytes = encode_announce("123456789", "Alice's laptop");
        assert_eq!(parse_announce(&bytes), Some(Announce { id: "123456789".into(), alias: "Alice's laptop".into() }));
    }

    #[test]
    fn a_long_alias_is_clamped_on_a_char_boundary() {
        // "é" is two bytes; 40 of them is 80 bytes, past MAX_ALIAS (64).
        let alias = "é".repeat(40);
        let bytes = encode_announce("1", &alias);
        let parsed = parse_announce(&bytes).unwrap();
        assert!(parsed.alias.len() <= MAX_ALIAS);
        assert!(alias.starts_with(&parsed.alias));
        // Not cut mid-character: it still decodes as whole "é"s.
        assert!(parsed.alias.chars().all(|c| c == 'é'));
    }

    #[test]
    fn junk_and_malformed_packets_are_rejected() {
        assert_eq!(parse_announce(b"hello"), None);
        assert_eq!(parse_announce(b"TDSC"), None); // no version
        assert_eq!(parse_announce(b"TDSC\x02\x01a\x00"), None); // wrong version
        assert_eq!(parse_announce(b"TDSC\x01\x09short\x00"), None); // id_len past the end
        assert_eq!(parse_announce(b"TDSC\x01\x00\x00"), None); // empty id
        // A valid packet whose alias bytes are then made invalid UTF-8.
        let mut bad = encode_announce("1", "ok");
        let last = bad.len() - 1;
        bad[last] = 0xFF;
        assert_eq!(parse_announce(&bad), None);
    }

    #[test]
    fn registry_keeps_the_latest_and_sorts_by_alias() {
        let mut registry = Registry::new(DEVICE_TTL);
        let base = Instant::now();
        registry.observe(Announce { id: "2".into(), alias: "Zed".into() }, addr(), base);
        registry.observe(Announce { id: "1".into(), alias: "Amy".into() }, addr(), base);
        // Re-announce id 2 with a new alias; it replaces, not duplicates.
        registry.observe(Announce { id: "2".into(), alias: "Zara".into() }, addr(), base);
        let devices = registry.devices(base);
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].alias, "Amy");
        assert_eq!(devices[1].alias, "Zara");
    }

    #[tokio::test]
    async fn start_then_stop_ends_the_worker() {
        let (discovery, worker) = Discovery::start("1".into(), "Test device".into());
        let handle = tokio::spawn(worker);
        assert!(discovery.devices().is_empty());
        discovery.stop();
        // The worker returns promptly after stop (or already did if the port
        // was busy and it couldn't bind).
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("worker should stop")
            .expect("worker should not panic");
    }

    #[test]
    fn devices_expire_after_the_ttl() {
        let mut registry = Registry::new(Duration::from_secs(10));
        let base = Instant::now();
        registry.observe(Announce { id: "1".into(), alias: "Amy".into() }, addr(), base);
        assert_eq!(registry.devices(base + Duration::from_secs(9)).len(), 1);
        assert_eq!(registry.devices(base + Duration::from_secs(11)).len(), 0);
        registry.prune(base + Duration::from_secs(11));
        assert_eq!(registry.devices(base).len(), 0);
    }
}
