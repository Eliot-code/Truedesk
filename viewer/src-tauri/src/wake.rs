//! Wake-on-LAN for the computers this app views. During a session each host
//! sends how to wake it (`Control::WakeInfo`); it is kept per device ID in
//! `wake.json`, so the computer can be woken once it sleeps: from this
//! computer's network, and through the host this app is viewing when that
//! host is on the sleeping computer's network.

use super::*;
use protocol::control::WakeTarget;
use serde::Deserialize;
use std::{collections::BTreeMap, time::{SystemTime, UNIX_EPOCH}};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct WakeEntry {
    targets: Vec<WakeTarget>,
    /// Unix seconds when the host last sent them.
    updated: u64,
}

pub(crate) struct WakeStore {
    path: PathBuf,
    devices: BTreeMap<String, WakeEntry>,
}

impl WakeStore {
    pub(crate) fn load(app_data_dir: impl AsRef<Path>) -> io::Result<Self> {
        let dir = app_data_dir.as_ref();
        fs::create_dir_all(dir)?;
        let path = dir.join("wake.json");
        let devices = match fs::read_to_string(&path) {
            Ok(contents) => serde_json::from_str(&contents).unwrap_or_else(|error| {
                match config::backup_corrupt_file(&path) {
                    Ok(backup) => log::warn!("could not parse wake file ({error}); backed it up to {}", backup.display()),
                    Err(backup_error) => log::warn!("could not parse wake file ({error}) or back it up ({backup_error})"),
                }
                BTreeMap::new()
            }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => return Err(error),
        };
        Ok(Self { path, devices })
    }

    /// Remember how to wake device `id`, replacing what was known.
    pub(crate) fn record(&mut self, id: &str, targets: Vec<WakeTarget>) -> io::Result<()> {
        let targets: Vec<WakeTarget> = targets.into_iter().take(protocol::control::MAX_WAKE_TARGETS).collect();
        if targets.is_empty() || self.devices.get(id).is_some_and(|entry| entry.targets == targets) {
            return Ok(());
        }
        let updated = SystemTime::now().duration_since(UNIX_EPOCH).map(|since| since.as_secs()).unwrap_or(0);
        self.devices.insert(id.to_owned(), WakeEntry { targets, updated });
        let bytes = serde_json::to_vec_pretty(&self.devices).map_err(io::Error::other)?;
        config::write_atomic(&self.path, &bytes)
    }

    pub(crate) fn targets(&self, id: &str) -> Option<Vec<WakeTarget>> {
        self.devices.get(id).map(|entry| entry.targets.clone())
    }

    /// The devices this app knows how to wake.
    pub(crate) fn ids(&self) -> Vec<String> {
        self.devices.keys().cloned().collect()
    }
}

/// What a wake request did.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub(crate) struct WakeResult {
    /// Packets sent from this computer's network.
    pub(crate) sent: usize,
    /// Also asked the host this app is viewing to send them on its networks.
    pub(crate) relayed: bool,
}

/// The host answered a relayed wake request.
#[derive(Serialize, Clone)]
pub(crate) struct WakeSentEvent {
    pub(crate) sent: u32,
}

/// The MAC addresses in `targets`, each once.
fn macs(targets: &[WakeTarget]) -> Vec<String> {
    let mut macs: Vec<String> = Vec::new();
    for target in targets {
        if !macs.contains(&target.mac) { macs.push(target.mac.clone()); }
    }
    macs
}

/// Wake device `peer_id`: send its magic packets from this computer, and ask
/// the host this app is viewing to send them too when it offers that.
#[tauri::command]
pub(crate) async fn wake_device(state: tauri::State<'_, AppState>, peer_id: String) -> Result<WakeResult, String> {
    let (targets, relay) = {
        let state = state.lock();
        (state.wake.targets(&peer_id), state.wake_relay)
    };
    let targets = targets.ok_or_else(|| "Truedesk doesn't know how to wake this device yet. Connect to it once while it's on.".to_owned())?;
    let sent = host_core::wol::wake(&targets).await;
    let relayed = relay && send_control(&state, Control::WakeOnLan { macs: macs(&targets) }).await.is_ok();
    if sent == 0 && !relayed {
        return Err("Couldn't send the wake-up signal. Check this computer's network connection.".into());
    }
    Ok(WakeResult { sent, relayed })
}

/// Whether the host this app is viewing wakes computers on its networks. The
/// UI asks when it opens, in case it missed the `wake_relay_support` event.
#[tauri::command]
pub(crate) fn wake_relay_support(state: tauri::State<'_, AppState>) -> bool {
    state.lock().wake_relay
}

/// The devices this app knows how to wake.
#[tauri::command]
pub(crate) fn wakeable_devices(state: tauri::State<'_, AppState>) -> Vec<String> {
    state.lock().wake.ids()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        std::env::temp_dir().join(format!("truedesk-wake-{label}-{}-{nanos}", std::process::id()))
    }

    fn target(mac: &str, broadcast: &str) -> WakeTarget {
        WakeTarget { mac: mac.into(), broadcast: broadcast.into() }
    }

    #[test]
    fn remembers_targets_across_loads() {
        let dir = temp_dir("roundtrip");
        let mut store = WakeStore::load(&dir).unwrap();
        assert!(store.ids().is_empty());
        store.record("123456789", vec![target("00:1a:2b:3c:4d:5e", "192.168.1.255")]).unwrap();
        store.record("987654321", Vec::new()).unwrap();
        let store = WakeStore::load(&dir).unwrap();
        assert_eq!(store.ids(), vec!["123456789".to_owned()]);
        assert_eq!(store.targets("123456789"), Some(vec![target("00:1a:2b:3c:4d:5e", "192.168.1.255")]));
        assert_eq!(store.targets("987654321"), None);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_corrupt_file_starts_over() {
        let dir = temp_dir("corrupt");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("wake.json"), b"{ not json").unwrap();
        let mut store = WakeStore::load(&dir).unwrap();
        assert!(store.ids().is_empty());
        store.record("123456789", vec![target("00:1a:2b:3c:4d:5e", "255.255.255.255")]).unwrap();
        assert_eq!(WakeStore::load(&dir).unwrap().ids(), vec!["123456789".to_owned()]);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn each_mac_is_relayed_once() {
        let targets = [target("aa:aa:aa:aa:aa:aa", "10.0.0.255"), target("aa:aa:aa:aa:aa:aa", "10.1.0.255"), target("bb:bb:bb:bb:bb:bb", "10.0.0.255")];
        assert_eq!(macs(&targets), vec!["aa:aa:aa:aa:aa:aa".to_owned(), "bb:bb:bb:bb:bb:bb".to_owned()]);
    }
}
