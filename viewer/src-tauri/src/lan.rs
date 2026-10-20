//! Finding other Truedesk devices on the local network, so you can connect by
//! name instead of typing a nine-digit ID. The engine lives in
//! `host_core::discovery`; this starts it with the device's ID and name, and
//! exposes the live list to the UI.
//!
//! Only the ID and the device name are broadcast on the LAN, never the screen
//! or any session data.

use super::*;
use host_core::discovery::{Device, Discovery};

/// A device seen on the local network, for the connect screen.
#[derive(Serialize, Clone)]
pub(crate) struct NearbyDevice {
    pub id: String,
    pub alias: String,
}

impl From<Device> for NearbyDevice {
    fn from(device: Device) -> Self {
        Self { id: device.id, alias: device.alias }
    }
}

/// The name this device shows on the network: the computer's hostname, or a
/// plain default when it has none.
pub(crate) fn device_alias() -> String {
    let name = gethostname::gethostname().to_string_lossy().trim().to_owned();
    if name.is_empty() {
        "Truedesk device".to_owned()
    } else {
        name
    }
}

/// Starts announcing this device and listening for others. The handle goes
/// into the app state; dropping it (on shutdown) stops discovery.
pub(crate) fn start(id: String) -> Discovery {
    let (discovery, worker) = Discovery::start(id, device_alias());
    tauri::async_runtime::spawn(worker);
    discovery
}

/// The devices seen on the LAN right now (excluding this one).
#[tauri::command]
pub(crate) fn nearby_devices(state: tauri::State<'_, AppState>) -> Vec<NearbyDevice> {
    match &state.lock().lan {
        Some(discovery) => discovery.devices().into_iter().map(NearbyDevice::from).collect(),
        None => Vec::new(),
    }
}
