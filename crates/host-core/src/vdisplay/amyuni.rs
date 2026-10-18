//! The parts of the Windows backend that don't touch Windows: facts about
//! Amyuni's usbmmidd_v2 driver, its display modes, and which display may
//! be removed when. Kept platform-neutral so they are tested everywhere.
//!
//! The driver (Amyuni Technologies, signed by Microsoft through the
//! Windows Hardware Compatibility Program) is an indirect display driver.
//! An `IOCTL` on its device interface adds a monitor or removes the newest
//! one; up to [`MAX_MONITORS`] at once, each offering the same fixed list
//! of modes from the registry. RustDesk drives it the same way
//! (`src/virtual_display_manager.rs`).

/// The driver's device interface, `{b5ffd75f-da40-4353-8ff8-b6daf6f1d8ca}`.
pub const INTERFACE_GUID: (u32, u16, u16, [u8; 8]) = (0xb5ff_d75f, 0xda40, 0x4353, [0x8f, 0xf8, 0xb6, 0xda, 0xf6, 0xf1, 0xd8, 0xca]);
/// `CTL_CODE(0x23, 0x903, METHOD_BUFFERED, FILE_WRITE_ACCESS)`.
pub const IOCTL_PLUG: u32 = 0x0023_340C;
/// The driver's hardware id, from its INF.
pub const HARDWARE_ID: &str = "usbmmidd";
/// What Windows calls the driver's display sources (`DeviceString`).
pub const DEVICE_STRING: &str = "USB Mobile Monitor Virtual Display";
/// Monitors the driver can show at once.
pub const MAX_MONITORS: usize = 4;
/// Mode slots in the registry (values "0" to "9").
pub const MODE_SLOTS: usize = 10;
/// The driver's mode list, relative to `HKEY_LOCAL_MACHINE`. Amyuni's
/// notes name the first; the driver's INF writes to its UMDF service key,
/// which is the second on current Windows. Whichever exists is used.
pub const MODE_KEYS: [&str; 2] = [
    r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\WUDF\Services\usbmmIdd\Parameters\Monitors",
    r"SYSTEM\CurrentControlSet\Services\WUDF\Services\usbmmIdd\Parameters\Monitors",
];

/// The modes Truedesk installs, common laptop and monitor sizes. The
/// driver's own list stops at 2560x1440 for 16:10 and has no ultra-wide.
pub const PRESET_MODES: [(u32, u32); MODE_SLOTS] = [
    (1280, 720),
    (1366, 768),
    (1600, 900),
    (1920, 1080),
    (1920, 1200),
    (2560, 1440),
    (2560, 1600),
    (2880, 1800),
    (3440, 1440),
    (3840, 2160),
];
/// The mode a new monitor starts in.
pub const DEFAULT_MODE: (u32, u32) = (1920, 1080);

/// The IOCTL's 4-byte input: add a monitor, or remove the newest.
pub fn plug_command(add: bool) -> [u8; 4] { [if add { 0x10 } else { 0x00 }, 0, 0, 0] }

/// Whether a display source's `DeviceString` is the driver's.
pub fn is_driver_source(device_string: &str) -> bool { device_string.trim_end_matches('\0').trim() == DEVICE_STRING }

/// Whether a device's hardware ids (a REG_MULTI_SZ, as bytes) include the
/// driver's.
pub fn has_hardware_id(multi_sz: &[u8]) -> bool {
    let units: Vec<u16> = multi_sz.as_chunks::<2>().0.iter().map(|&pair| u16::from_le_bytes(pair)).collect();
    units
        .split(|&unit| unit == 0)
        .take_while(|id| !id.is_empty())
        .any(|id| String::from_utf16_lossy(id).eq_ignore_ascii_case(HARDWARE_ID))
}

/// Reads a registry mode, `"1920,1080"`.
pub fn parse_mode(value: &str) -> Option<(u32, u32)> {
    let (width, height) = value.trim().trim_end_matches('\0').split_once(',')?;
    let mode = (width.trim().parse().ok()?, height.trim().parse().ok()?);
    (mode.0 > 0 && mode.1 > 0).then_some(mode)
}

/// The registry values for `modes`: `(name, data)` pairs, the default
/// value (empty name) first.
pub fn mode_values(modes: &[(u32, u32)], default: (u32, u32)) -> Vec<(String, String)> {
    let format = |(width, height): (u32, u32)| format!("{width},{height}");
    std::iter::once((String::new(), format(default)))
        .chain(modes.iter().take(MODE_SLOTS).enumerate().map(|(slot, &mode)| (slot.to_string(), format(mode))))
        .collect()
}

/// The mode nearest to `width` x `height`: least off in each direction,
/// measured as a ratio so a 10% miss counts the same at any size.
pub fn closest_mode(modes: &[(u32, u32)], width: u32, height: u32) -> Option<(u32, u32)> {
    let off = |want: u32, have: u32| (f64::from(have.max(1)) / f64::from(want.max(1))).ln().abs();
    modes
        .iter()
        .copied()
        .filter(|&(w, h)| w > 0 && h > 0)
        .min_by(|&a, &b| {
            let cost = |(w, h): (u32, u32)| off(width, w) + off(height, h);
            cost(a).total_cmp(&cost(b))
        })
}

/// A display this process added through the driver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Added {
    /// The id handed out (the `HMONITOR` when it came up).
    pub id: u32,
    /// Its display source, `\\.\DISPLAYn`.
    pub source: String,
    /// The driver's sources with a monitor just before this one was added.
    pub before: Vec<String>,
}

/// How to remove a display, when it may be removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Removal {
    /// Send the remove IOCTL: it is the driver's newest monitor.
    Unplug,
    /// It is already gone (someone else removed it); just forget it.
    Forget,
}

/// Why a display may not be removed now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Not one this process added.
    Unknown,
    /// Something was added after it, by this process or anyone else; the
    /// driver would take that one instead.
    NotNewest,
}

/// The displays this process added, oldest first.
///
/// The driver only removes its newest monitor, and other programs (or the
/// user, with Amyuni's own tool) can add and remove monitors too, so a
/// display is only removed when it is the last this process added and no
/// monitor appeared after it that isn't ours.
#[derive(Debug, Default)]
pub struct Ledger {
    added: Vec<Added>,
}

impl Ledger {
    pub fn push(&mut self, added: Added) { self.added.push(added); }

    pub fn ids(&self) -> Vec<u32> { self.added.iter().map(|added| added.id).collect() }

    /// Whether display `id` may go, given the driver's sources that have a
    /// monitor now.
    pub fn check_remove(&self, id: u32, present: &[String]) -> Result<Removal, Refusal> {
        let index = self.added.iter().position(|added| added.id == id).ok_or(Refusal::Unknown)?;
        let entry = &self.added[index];
        if !present.contains(&entry.source) {
            return Ok(Removal::Forget);
        }
        // A later display of ours that is still there sits on top of it.
        if self.added[index + 1..].iter().any(|later| present.contains(&later.source)) {
            return Err(Refusal::NotNewest);
        }
        // So does one somebody else added after it.
        let newer = present.iter().any(|source| *source != entry.source && !entry.before.contains(source));
        if newer {
            return Err(Refusal::NotNewest);
        }
        Ok(Removal::Unplug)
    }

    /// Drops display `id` from the ledger.
    pub fn forget(&mut self, id: u32) -> Option<Added> {
        let index = self.added.iter().position(|added| added.id == id)?;
        Some(self.added.remove(index))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn added(id: u32, source: &str, before: &[&str]) -> Added {
        Added { id, source: source.into(), before: before.iter().map(|&name| name.into()).collect() }
    }

    fn present(names: &[&str]) -> Vec<String> { names.iter().map(|&name| name.into()).collect() }

    #[test]
    fn driver_constants_match_rustdesk() {
        // RustDesk sends 2307084 with [0x10, 0, 0, 0] to add, [0, 0, 0, 0] to remove.
        assert_eq!(IOCTL_PLUG, 2_307_084);
        assert_eq!(plug_command(true), [0x10, 0, 0, 0]);
        assert_eq!(plug_command(false), [0, 0, 0, 0]);
        assert_eq!(INTERFACE_GUID.0, 0xb5ff_d75f);
        assert!(is_driver_source("USB Mobile Monitor Virtual Display\0\0\0"));
        assert!(!is_driver_source("NVIDIA GeForce RTX 4070"));
    }

    #[test]
    fn hardware_ids_are_matched_in_a_multi_string() {
        let multi = |ids: &[&str]| -> Vec<u8> {
            ids.iter().flat_map(|id| id.encode_utf16().chain([0])).chain([0]).flat_map(u16::to_le_bytes).collect()
        };
        assert!(has_hardware_id(&multi(&["usbmmidd"])));
        assert!(has_hardware_id(&multi(&["ROOT\\OTHER", "USBMMIDD"])));
        assert!(!has_hardware_id(&multi(&["usbmmidd2"])));
        // Anything after the empty string that ends the list is ignored.
        let mut trailing = multi(&["ROOT\\OTHER"]);
        trailing.extend(multi(&["usbmmidd"]));
        assert!(!has_hardware_id(&trailing));
        assert!(!has_hardware_id(&[0; 16]));
    }

    #[test]
    fn presets_fit_the_driver_and_the_encoder() {
        assert_eq!(PRESET_MODES.len(), MODE_SLOTS);
        assert!(PRESET_MODES.contains(&DEFAULT_MODE));
        for (width, height) in PRESET_MODES {
            assert!(width % 2 == 0 && height % 2 == 0, "{width}x{height} must be even for H.264");
            assert!(width <= super::super::MAX_WIDTH && height <= super::super::MAX_HEIGHT);
        }
    }

    #[test]
    fn mode_values_are_what_the_inf_writes() {
        let values = mode_values(&PRESET_MODES, DEFAULT_MODE);
        assert_eq!(values.len(), 11);
        assert_eq!(values[0], (String::new(), "1920,1080".to_owned()));
        assert_eq!(values[1], ("0".to_owned(), "1280,720".to_owned()));
        assert_eq!(values[10], ("9".to_owned(), "3840,2160".to_owned()));
        // Never more than ten slots.
        let many = vec![(800, 600); 14];
        assert_eq!(mode_values(&many, DEFAULT_MODE).len(), 11);
    }

    #[test]
    fn modes_parse_like_the_driver_writes_them() {
        assert_eq!(parse_mode("1920,1080"), Some((1920, 1080)));
        assert_eq!(parse_mode(" 2560 , 1440\0"), Some((2560, 1440)));
        assert_eq!(parse_mode("1920x1080"), None);
        assert_eq!(parse_mode("0,1080"), None);
        assert_eq!(parse_mode(""), None);
    }

    #[test]
    fn requests_snap_to_the_nearest_mode() {
        let modes = PRESET_MODES;
        assert_eq!(closest_mode(&modes, 1920, 1080), Some((1920, 1080)));
        assert_eq!(closest_mode(&modes, 1910, 1046), Some((1920, 1080)));
        // A 16:10 laptop window.
        assert_eq!(closest_mode(&modes, 2880, 1764), Some((2880, 1800)));
        assert_eq!(closest_mode(&modes, 1440, 900), Some((1600, 900)));
        // Ultra-wide.
        assert_eq!(closest_mode(&modes, 3440, 1400), Some((3440, 1440)));
        // Tiny and huge requests get the ends of the list.
        assert_eq!(closest_mode(&modes, 640, 480), Some((1280, 720)));
        assert_eq!(closest_mode(&modes, 7680, 4320), Some((3840, 2160)));
        assert_eq!(closest_mode(&[], 1920, 1080), None);
    }

    #[test]
    fn only_the_newest_display_comes_off() {
        let mut ledger = Ledger::default();
        ledger.push(added(11, r"\\.\DISPLAY5", &[]));
        ledger.push(added(12, r"\\.\DISPLAY6", &[r"\\.\DISPLAY5"]));
        let both = present(&[r"\\.\DISPLAY5", r"\\.\DISPLAY6"]);
        assert_eq!(ledger.check_remove(11, &both), Err(Refusal::NotNewest));
        assert_eq!(ledger.check_remove(12, &both), Ok(Removal::Unplug));
        assert_eq!(ledger.check_remove(99, &both), Err(Refusal::Unknown));
        ledger.forget(12);
        assert_eq!(ledger.check_remove(11, &present(&[r"\\.\DISPLAY5"])), Ok(Removal::Unplug));
        assert_eq!(ledger.ids(), vec![11]);
    }

    #[test]
    fn displays_other_programs_added_are_never_taken() {
        let mut ledger = Ledger::default();
        // The user had one of their own before Truedesk added one.
        ledger.push(added(21, r"\\.\DISPLAY4", &[r"\\.\DISPLAY3"]));
        assert_eq!(ledger.check_remove(21, &present(&[r"\\.\DISPLAY3", r"\\.\DISPLAY4"])), Ok(Removal::Unplug));
        // Then they added another on top: removing now would take theirs.
        let crowded = present(&[r"\\.\DISPLAY3", r"\\.\DISPLAY4", r"\\.\DISPLAY7"]);
        assert_eq!(ledger.check_remove(21, &crowded), Err(Refusal::NotNewest));
    }

    #[test]
    fn displays_removed_behind_our_back_are_just_forgotten() {
        let mut ledger = Ledger::default();
        ledger.push(added(31, r"\\.\DISPLAY5", &[]));
        ledger.push(added(32, r"\\.\DISPLAY6", &[r"\\.\DISPLAY5"]));
        // Amyuni's tool removed the newest; ours below it is free again.
        let left = present(&[r"\\.\DISPLAY5"]);
        assert_eq!(ledger.check_remove(32, &left), Ok(Removal::Forget));
        assert_eq!(ledger.check_remove(31, &left), Ok(Removal::Unplug));
        assert_eq!(ledger.forget(32).map(|added| added.id), Some(32));
        assert_eq!(ledger.forget(32), None);
    }
}
