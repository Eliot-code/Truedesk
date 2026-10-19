//! Wake-on-LAN: the magic packets that wake a sleeping computer, and the
//! addresses this computer can be woken on.
//!
//! A magic packet is six 0xFF bytes followed by the target's MAC address 16
//! times, sent as a UDP broadcast on the target's network. During a session a
//! host tells its viewer how to wake it ([`local_targets`]), so the viewer can
//! wake it later: directly when they share a network ([`wake`]), or through
//! another Truedesk host on that network ([`wake_macs`] on that host).

use protocol::control::{WakeTarget, MAX_WAKE_TARGETS};
use std::{net::Ipv4Addr, time::Duration};
use tokio::net::UdpSocket;

/// The UDP port magic packets go to (the discard service), as most tools use.
pub const WAKE_PORT: u16 = 9;
/// Each packet goes out this many times, since UDP may drop one.
const ROUNDS: usize = 3;
const ROUND_GAP: Duration = Duration::from_millis(100);

/// `aa:bb:cc:dd:ee:ff` or `AA-BB-CC-DD-EE-FF`.
pub fn parse_mac(text: &str) -> Option<[u8; 6]> {
    let parts: Vec<&str> = text.trim().split([':', '-']).collect();
    if parts.len() != 6 {
        return None;
    }
    let mut mac = [0u8; 6];
    for (byte, part) in mac.iter_mut().zip(parts) {
        if part.len() != 2 {
            return None;
        }
        *byte = u8::from_str_radix(part, 16).ok()?;
    }
    Some(mac)
}

/// `aa:bb:cc:dd:ee:ff`.
pub fn format_mac(mac: [u8; 6]) -> String {
    mac.iter().map(|byte| format!("{byte:02x}")).collect::<Vec<_>>().join(":")
}

pub fn magic_packet(mac: [u8; 6]) -> [u8; 102] {
    let mut packet = [0xFF; 102];
    let (copies, _) = packet[6..].as_chunks_mut::<6>();
    for copy in copies {
        *copy = mac;
    }
    packet
}

/// A card address a packet can wake: set, and not a group address.
fn wakeable(mac: [u8; 6]) -> bool {
    mac != [0; 6] && mac[0] & 1 == 0
}

/// The broadcast address of the network `address` is on; the limited
/// broadcast when the network has no room for one (/0, /31, /32).
fn network_broadcast(address: Ipv4Addr, mask: Ipv4Addr) -> Ipv4Addr {
    let mask = u32::from(mask);
    if mask == 0 || mask.leading_ones() >= 31 {
        return Ipv4Addr::BROADCAST;
    }
    Ipv4Addr::from(u32::from(address) | !mask)
}

/// A network card that may wake this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
struct NetInterface {
    name: String,
    mac: [u8; 6],
    /// IPv4 addresses with their netmasks.
    ipv4: Vec<(Ipv4Addr, Ipv4Addr)>,
}

/// One target per IPv4 network of each card, or the limited broadcast for a
/// card without one (say, a cable that isn't plugged in right now).
fn targets_for(interfaces: &[NetInterface]) -> Vec<WakeTarget> {
    let mut targets: Vec<WakeTarget> = Vec::new();
    for interface in interfaces.iter().filter(|interface| wakeable(interface.mac)) {
        let broadcasts: Vec<Ipv4Addr> = if interface.ipv4.is_empty() {
            vec![Ipv4Addr::BROADCAST]
        } else {
            interface.ipv4.iter().map(|(address, mask)| network_broadcast(*address, *mask)).collect()
        };
        for broadcast in broadcasts {
            let target = WakeTarget { mac: format_mac(interface.mac), broadcast: broadcast.to_string() };
            if !targets.contains(&target) {
                targets.push(target);
            }
        }
    }
    targets.truncate(MAX_WAKE_TARGETS);
    targets
}

/// How to wake this computer: its network cards and their networks.
pub fn local_targets() -> Vec<WakeTarget> {
    targets_for(&interfaces())
}

/// Wake each target: its packet goes to the target's network broadcast and
/// to the limited broadcast. Returns how many packets went out (each is also
/// repeated a few times); zero when none could be sent.
pub async fn wake(targets: &[WakeTarget]) -> usize {
    wake_on(targets, WAKE_PORT).await
}

/// The host side of a relayed wake: wake the cards with these MAC addresses
/// on every network this computer is on. Destinations are always this
/// computer's own broadcast addresses, never addresses from the viewer.
pub async fn wake_macs(macs: &[String]) -> usize {
    let mut networks: Vec<String> = local_targets().into_iter().map(|target| target.broadcast).collect();
    networks.sort();
    networks.dedup();
    let targets: Vec<WakeTarget> = macs
        .iter()
        .take(MAX_WAKE_TARGETS)
        .flat_map(|mac| {
            let networks = if networks.is_empty() { vec![Ipv4Addr::BROADCAST.to_string()] } else { networks.clone() };
            networks.into_iter().map(move |broadcast| WakeTarget { mac: mac.clone(), broadcast })
        })
        .collect();
    wake_on(&targets, WAKE_PORT).await
}

async fn wake_on(targets: &[WakeTarget], port: u16) -> usize {
    let mut packets: Vec<([u8; 102], Ipv4Addr)> = Vec::new();
    for target in targets {
        let Some(mac) = parse_mac(&target.mac).filter(|mac| wakeable(*mac)) else { continue };
        let packet = magic_packet(mac);
        let network = target.broadcast.parse::<Ipv4Addr>().ok().filter(|address| !address.is_unspecified() && !address.is_multicast());
        for destination in [network, Some(Ipv4Addr::BROADCAST)].into_iter().flatten() {
            if !packets.contains(&(packet, destination)) {
                packets.push((packet, destination));
            }
        }
    }
    if packets.is_empty() {
        return 0;
    }
    let Ok(socket) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await else { return 0 };
    if socket.set_broadcast(true).is_err() {
        return 0;
    }
    let mut sent = 0;
    for round in 0..ROUNDS {
        if round > 0 {
            tokio::time::sleep(ROUND_GAP).await;
        }
        for (packet, destination) in &packets {
            let delivered = socket.send_to(packet, (*destination, port)).await.is_ok();
            if round == 0 && delivered {
                sent += 1;
            }
        }
    }
    sent
}

/// Network cards from `/sys/class/net`, with their IPv4 networks.
#[cfg(target_os = "linux")]
fn interfaces() -> Vec<NetInterface> {
    use std::{fs, net::SocketAddr};
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir("/sys/class/net") else { return found };
    for entry in entries.flatten() {
        let path = entry.path();
        // Real cards, wired or wireless, have a backing device; the loopback,
        // bridges, veths, tunnels and other virtual interfaces don't.
        if !path.join("device").exists() {
            continue;
        }
        // ARPHRD_ETHER: Ethernet and Wi-Fi.
        if fs::read_to_string(path.join("type")).ok().as_deref().map(str::trim) != Some("1") {
            continue;
        }
        let Some(mac) = fs::read_to_string(path.join("address")).ok().and_then(|text| parse_mac(&text)) else { continue };
        found.push(NetInterface { name: entry.file_name().to_string_lossy().into_owned(), mac, ipv4: Vec::new() });
    }
    for address in webrtc_util::ifaces::ifaces().unwrap_or_default() {
        let (Some(SocketAddr::V4(ip)), Some(SocketAddr::V4(mask))) = (address.addr, address.mask) else { continue };
        if let Some(interface) = found.iter_mut().find(|interface| interface.name == address.name) {
            interface.ipv4.push((*ip.ip(), *mask.ip()));
        }
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

/// Ethernet and Wi-Fi adapters from `GetAdaptersAddresses`, with their IPv4
/// networks.
#[cfg(windows)]
fn interfaces() -> Vec<NetInterface> {
    use windows::Win32::{
        Foundation::{ERROR_BUFFER_OVERFLOW, NO_ERROR},
        NetworkManagement::IpHelper::{
            GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST, IF_TYPE_ETHERNET_CSMACD, IF_TYPE_IEEE80211, IP_ADAPTER_ADDRESSES_LH,
        },
        Networking::WinSock::{AF_INET, SOCKADDR_IN},
    };
    // Virtual switches, VPNs and tunnels report themselves as Ethernet; their
    // descriptions give them away.
    const VIRTUAL: [&str; 12] = ["virtual", "hyper-v", "vpn", "tap-", "vmware", "virtualbox", "loopback", "bluetooth", "miniport", "pseudo", "npcap", "wsl"];

    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    let mut size: u32 = 16 * 1024;
    // u64 words keep the buffer aligned for the adapter structs.
    let mut buffer: Vec<u64> = Vec::new();
    let mut filled = false;
    for _ in 0..4 {
        buffer = vec![0; (size as usize).div_ceil(8)];
        // SAFETY: `buffer` holds `size` writable, suitably aligned bytes.
        let result = unsafe { GetAdaptersAddresses(u32::from(AF_INET.0), flags, None, Some(buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>()), &mut size) };
        if result == ERROR_BUFFER_OVERFLOW.0 {
            continue; // `size` now says how much it needs.
        }
        filled = result == NO_ERROR.0;
        break;
    }
    if !filled {
        return Vec::new();
    }
    let mut found = Vec::new();
    // SAFETY: on success the buffer starts with a linked list of adapters
    // (and their addresses) that stays valid while `buffer` lives.
    let mut adapter = buffer.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
    while let Some(entry) = unsafe { adapter.as_ref() } {
        adapter = entry.Next.cast_const();
        if !matches!(entry.IfType, IF_TYPE_ETHERNET_CSMACD | IF_TYPE_IEEE80211) || entry.PhysicalAddressLength != 6 || entry.Description.is_null() {
            continue;
        }
        let description = unsafe { entry.Description.to_string() }.unwrap_or_default().to_lowercase();
        if VIRTUAL.iter().any(|word| description.contains(word)) {
            continue;
        }
        let mut mac = [0u8; 6];
        mac.copy_from_slice(&entry.PhysicalAddress[..6]);
        let mut ipv4 = Vec::new();
        let mut unicast = entry.FirstUnicastAddress;
        while let Some(address) = unsafe { unicast.as_ref() } {
            unicast = address.Next;
            let sockaddr = address.Address.lpSockaddr;
            if sockaddr.is_null() || unsafe { (*sockaddr).sa_family } != AF_INET {
                continue;
            }
            // SAFETY: an AF_INET address is a SOCKADDR_IN; S_addr holds the
            // address in network byte order, as its in-memory bytes.
            let ip = Ipv4Addr::from(unsafe { (*sockaddr.cast::<SOCKADDR_IN>()).sin_addr.S_un.S_addr }.to_ne_bytes());
            let prefix = u32::from(address.OnLinkPrefixLength.min(32));
            let mask = if prefix == 0 { 0 } else { u32::MAX << (32 - prefix) };
            ipv4.push((ip, Ipv4Addr::from(mask)));
        }
        let name = if entry.FriendlyName.is_null() { String::new() } else { unsafe { entry.FriendlyName.to_string() }.unwrap_or_default() };
        found.push(NetInterface { name, mac, ipv4 });
    }
    found
}

/// Other platforms don't report wake addresses yet.
#[cfg(not(any(target_os = "linux", windows)))]
fn interfaces() -> Vec<NetInterface> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAC: [u8; 6] = [0x00, 0x1a, 0x2b, 0x3c, 0x4d, 0x5e];

    #[test]
    fn macs_parse_and_format() {
        assert_eq!(parse_mac("00:1a:2b:3c:4d:5e"), Some(MAC));
        assert_eq!(parse_mac(" 00-1A-2B-3C-4D-5E\n"), Some(MAC));
        assert_eq!(parse_mac("00:1a:2b:3c:4d"), None);
        assert_eq!(parse_mac("00:1a:2b:3c:4d:5e:6f"), None);
        assert_eq!(parse_mac("0:1a:2b:3c:4d:5e"), None);
        assert_eq!(parse_mac("zz:1a:2b:3c:4d:5e"), None);
        assert_eq!(format_mac(MAC), "00:1a:2b:3c:4d:5e");
    }

    #[test]
    fn a_magic_packet_is_sync_then_sixteen_macs() {
        let packet = magic_packet(MAC);
        assert_eq!(packet[..6], [0xFF; 6]);
        assert_eq!(packet.len(), 6 + 16 * 6);
        let (copies, rest) = packet[6..].as_chunks::<6>();
        assert!(rest.is_empty() && copies.len() == 16 && copies.iter().all(|copy| *copy == MAC));
    }

    #[test]
    fn broadcasts_follow_the_netmask() {
        let address = Ipv4Addr::new(192, 168, 1, 37);
        assert_eq!(network_broadcast(address, Ipv4Addr::new(255, 255, 255, 0)), Ipv4Addr::new(192, 168, 1, 255));
        assert_eq!(network_broadcast(address, Ipv4Addr::new(255, 255, 240, 0)), Ipv4Addr::new(192, 168, 15, 255));
        assert_eq!(network_broadcast(address, Ipv4Addr::new(255, 255, 255, 255)), Ipv4Addr::BROADCAST);
        assert_eq!(network_broadcast(address, Ipv4Addr::new(255, 255, 255, 254)), Ipv4Addr::BROADCAST);
        assert_eq!(network_broadcast(address, Ipv4Addr::UNSPECIFIED), Ipv4Addr::BROADCAST);
    }

    #[test]
    fn targets_cover_each_network_once_and_skip_unwakeable_cards() {
        let netmask = Ipv4Addr::new(255, 255, 255, 0);
        let interfaces = [
            NetInterface { name: "eth0".into(), mac: MAC, ipv4: vec![(Ipv4Addr::new(10, 0, 0, 5), netmask), (Ipv4Addr::new(10, 0, 0, 6), netmask), (Ipv4Addr::new(192, 168, 7, 2), netmask)] },
            NetInterface { name: "wlan0".into(), mac: [0x02, 1, 2, 3, 4, 5], ipv4: Vec::new() },
            NetInterface { name: "zero".into(), mac: [0; 6], ipv4: Vec::new() },
            NetInterface { name: "group".into(), mac: [0x01, 0, 0x5e, 0, 0, 1], ipv4: Vec::new() },
        ];
        let target = |mac: &str, broadcast: &str| WakeTarget { mac: mac.into(), broadcast: broadcast.into() };
        assert_eq!(targets_for(&interfaces), vec![
            target("00:1a:2b:3c:4d:5e", "10.0.0.255"),
            target("00:1a:2b:3c:4d:5e", "192.168.7.255"),
            target("02:01:02:03:04:05", "255.255.255.255"),
        ]);
        let many: Vec<NetInterface> = (0..40u8).map(|index| NetInterface { name: format!("eth{index}"), mac: [0, 0, 0, 0, 0, index + 1], ipv4: Vec::new() }).collect();
        assert_eq!(targets_for(&many).len(), MAX_WAKE_TARGETS);
    }

    #[tokio::test]
    async fn wake_sends_the_magic_packet_to_the_target_network() {
        let listener = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        // Loopback stands in for the target's network broadcast here.
        let targets = [WakeTarget { mac: format_mac(MAC), broadcast: "127.0.0.1".into() }];
        assert!(wake_on(&targets, port).await >= 1);
        let mut packet = [0u8; 200];
        let (length, _) = tokio::time::timeout(Duration::from_secs(5), listener.recv_from(&mut packet)).await.expect("packet in time").unwrap();
        assert_eq!(packet[..length], magic_packet(MAC));
    }

    #[tokio::test]
    async fn nothing_is_sent_for_unusable_targets() {
        let targets = [
            WakeTarget { mac: "not a mac".into(), broadcast: "127.0.0.1".into() },
            WakeTarget { mac: "00:00:00:00:00:00".into(), broadcast: "127.0.0.1".into() },
        ];
        assert_eq!(wake_on(&targets, WAKE_PORT).await, 0);
        assert_eq!(wake(&[]).await, 0);
    }

    #[test]
    fn local_targets_are_well_formed() {
        for target in local_targets() {
            assert!(parse_mac(&target.mac).is_some_and(wakeable), "{target:?}");
            assert!(target.broadcast.parse::<Ipv4Addr>().is_ok(), "{target:?}");
        }
    }
}
