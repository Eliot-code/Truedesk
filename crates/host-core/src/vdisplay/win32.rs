//! Virtual displays on Windows through Amyuni's usbmmidd_v2 indirect
//! display driver, which the user installs from Home ("Install virtual
//! display driver"). [`super::amyuni`] has the driver's interface and the
//! logic that doesn't need Windows.
//!
//! Adding a display: send the driver's add IOCTL, wait for one of its
//! display sources (`\\.\DISPLAYn`, from `EnumDisplayDevicesW`) to gain a
//! monitor, set that source to the driver mode closest to the request
//! (`ChangeDisplaySettingsExW`), then find the `HMONITOR` whose `szDevice`
//! is that source (`EnumDisplayMonitors` + `GetMonitorInfoW`). The
//! `HMONITOR` is the display's id: scap (through windows-capture) lists
//! monitors as `HMONITOR as u32`, and input injection turns ids back into
//! `HMONITOR`s the same way. The source name is what the backend keeps to
//! find the display again.

use super::amyuni::{self, Added, Ledger, Refusal, Removal};
use super::{AddedDisplay, DisplaySpec, VirtualDisplayError, VirtualDisplays};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};
use windows::core::{BOOL, GUID, PCWSTR};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW, DIGCF_DEVICEINTERFACE,
    DIGCF_PRESENT, HDEVINFO, SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
};
use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, GENERIC_READ, GENERIC_WRITE, LPARAM, POINTL, RECT};
use windows::Win32::Graphics::Gdi::{
    ChangeDisplaySettingsExW, EnumDisplayDevicesW, EnumDisplayMonitors, EnumDisplaySettingsW, GetMonitorInfoW, CDS_NORESET, CDS_TYPE,
    CDS_UPDATEREGISTRY, DEVMODEW, DISPLAY_DEVICEW, DISPLAY_DEVICE_ATTACHED_TO_DESKTOP, DISP_CHANGE_SUCCESSFUL, DM_PELSHEIGHT, DM_PELSWIDTH,
    DM_POSITION, ENUM_CURRENT_SETTINGS, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_XVIRTUALSCREEN};

const INTERFACE: GUID = GUID::from_values(amyuni::INTERFACE_GUID.0, amyuni::INTERFACE_GUID.1, amyuni::INTERFACE_GUID.2, amyuni::INTERFACE_GUID.3);
/// How long the driver gets to bring up a monitor after the IOCTL.
const PLUG_WAIT: Duration = Duration::from_secs(5);
/// How long Windows gets to put the new monitor on the desktop.
const DESKTOP_WAIT: Duration = Duration::from_secs(3);
const POLL: Duration = Duration::from_millis(50);

/// The backend: displays this process added, in order.
pub struct AmyuniDisplays {
    ledger: Mutex<Ledger>,
}

impl AmyuniDisplays {
    pub fn new() -> Self { Self { ledger: Mutex::new(Ledger::default()) } }

    fn ledger(&self) -> MutexGuard<'_, Ledger> { self.ledger.lock().unwrap_or_else(|poison| poison.into_inner()) }
}

impl Default for AmyuniDisplays {
    fn default() -> Self { Self::new() }
}

impl VirtualDisplays for AmyuniDisplays {
    fn supported(&self) -> bool { device_path().is_some() }

    fn create(&self, spec: &DisplaySpec) -> Result<AddedDisplay, VirtualDisplayError> {
        // Held throughout, so two sessions never race for the same new source.
        let mut ledger = self.ledger();
        if device_path().is_none() {
            return Err(VirtualDisplayError::Unsupported);
        }
        let before = driver_sources();
        if before.len() >= amyuni::MAX_MONITORS {
            return Err(VirtualDisplayError::Limit);
        }
        plug(true).map_err(VirtualDisplayError::Failed)?;
        let is_new = |source: &Source| !before.iter().any(|old| old.name == source.name);
        let Some(source) = wait_for(PLUG_WAIT, || driver_sources().into_iter().find(is_new)) else {
            return Err(VirtualDisplayError::Failed("the driver accepted the request but no display came up".into()));
        };
        let mode = amyuni::closest_mode(&installed_modes(), spec.width, spec.height).unwrap_or(amyuni::DEFAULT_MODE);
        // Windows normally extends the desktop onto a new monitor; if it
        // didn't, place this one to the right of everything else.
        let position = (!source.attached).then(right_of_desktop);
        if let Err(error) = set_mode(&source.name, mode, position) {
            log::warn!("could not set {} to {}x{}: {error}", source.name, mode.0, mode.1);
        }
        let Some(id) = wait_for(DESKTOP_WAIT, || monitor_id(&source.name)) else {
            // Take it away again, unless something else came up meanwhile
            // (then it may not be the newest).
            let added: Vec<_> = driver_sources().into_iter().filter(is_new).collect();
            if added.len() == 1 && added[0].name == source.name {
                let _ = plug(false);
            }
            return Err(VirtualDisplayError::Failed(format!("{} never joined the desktop", source.name)));
        };
        let (width, height) = current_mode(&source.name).unwrap_or(mode);
        log::info!("added virtual display {id} on {} at {width}x{height} (asked for {}x{})", source.name, spec.width, spec.height);
        ledger.push(Added { id, source: source.name, before: before.into_iter().map(|source| source.name).collect() });
        Ok(AddedDisplay { id, width, height })
    }

    fn removable(&self, id: u32) -> Result<(), VirtualDisplayError> {
        match self.ledger().check_remove(id, &source_names()) {
            Ok(_) | Err(Refusal::Unknown) => Ok(()),
            Err(Refusal::NotNewest) => Err(VirtualDisplayError::NotNewest),
        }
    }

    fn remove(&self, id: u32) -> Result<(), VirtualDisplayError> {
        let mut ledger = self.ledger();
        match ledger.check_remove(id, &source_names()) {
            Err(Refusal::Unknown) => Ok(()),
            Err(Refusal::NotNewest) => Err(VirtualDisplayError::NotNewest),
            Ok(Removal::Forget) => {
                ledger.forget(id);
                Ok(())
            }
            Ok(Removal::Unplug) => {
                plug(false).map_err(VirtualDisplayError::Failed)?;
                if let Some(added) = ledger.forget(id) {
                    log::info!("removed virtual display {id} ({})", added.source);
                }
                Ok(())
            }
        }
    }

    fn owned(&self) -> Vec<u32> { self.ledger().ids() }
}

fn wait_for<T>(limit: Duration, mut probe: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(found) = probe() {
            return Some(found);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(POLL);
    }
}

pub(crate) fn wide(text: &str) -> Vec<u16> { text.encode_utf16().chain(Some(0)).collect() }

pub(crate) fn from_wide(buffer: &[u16]) -> String {
    let end = buffer.iter().position(|&unit| unit == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

/// A device information set, destroyed on drop.
pub(crate) struct DeviceSet(pub(crate) HDEVINFO);

impl Drop for DeviceSet {
    fn drop(&mut self) {
        // SAFETY: the set came from SetupDiGetClassDevsW or
        // SetupDiCreateDeviceInfoList and is destroyed once.
        let _ = unsafe { SetupDiDestroyDeviceInfoList(self.0) };
    }
}

/// The driver's device interface path, when the driver is installed and
/// running.
fn device_path() -> Option<Vec<u16>> {
    // SAFETY: standard SetupAPI enumeration. The detail buffer is u64
    // aligned, at least `required` bytes, and starts with a correctly
    // sized header; the path is read within the bytes Windows filled.
    unsafe {
        let set = DeviceSet(SetupDiGetClassDevsW(Some(&INTERFACE), PCWSTR::null(), None, DIGCF_PRESENT | DIGCF_DEVICEINTERFACE).ok()?);
        let mut interface = SP_DEVICE_INTERFACE_DATA { cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32, ..Default::default() };
        SetupDiEnumDeviceInterfaces(set.0, None, &INTERFACE, 0, &mut interface).ok()?;
        let mut required = 0u32;
        // Fails with ERROR_INSUFFICIENT_BUFFER and reports the size.
        let _ = SetupDiGetDeviceInterfaceDetailW(set.0, &interface, None, 0, Some(&mut required), None);
        let header = std::mem::offset_of!(SP_DEVICE_INTERFACE_DETAIL_DATA_W, DevicePath);
        if (required as usize) <= header {
            return None;
        }
        let mut buffer = vec![0u64; (required as usize).div_ceil(8)];
        let detail = buffer.as_mut_ptr().cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
        // The C struct's size: 8 on 64-bit, 6 on 32-bit (no tail padding).
        (*detail).cbSize = if cfg!(target_pointer_width = "64") { 8 } else { 6 };
        SetupDiGetDeviceInterfaceDetailW(set.0, &interface, Some(detail), required, None, None).ok()?;
        let units = (required as usize - header) / 2;
        let path = std::slice::from_raw_parts(std::ptr::addr_of!((*detail).DevicePath).cast::<u16>(), units);
        let end = path.iter().position(|&unit| unit == 0)?;
        Some(path[..=end].to_vec())
    }
}

/// Sends the driver's IOCTL: add a monitor, or remove its newest.
fn plug(add: bool) -> Result<(), String> {
    let path = device_path().ok_or("the virtual display driver isn't running")?;
    let command = amyuni::plug_command(add);
    // SAFETY: `path` is NUL-terminated; the input buffer outlives the
    // synchronous call; the handle is closed once.
    unsafe {
        let device = CreateFileW(
            PCWSTR(path.as_ptr()),
            GENERIC_READ.0 | GENERIC_WRITE.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
        .map_err(|error| format!("could not open the driver: {error}"))?;
        let mut returned = 0u32;
        let sent = DeviceIoControl(device, amyuni::IOCTL_PLUG, Some(command.as_ptr().cast()), command.len() as u32, None, 0, Some(&mut returned), None);
        let _ = CloseHandle(device);
        sent.map_err(|error| format!("the driver refused to {} a display: {error}", if add { "add" } else { "remove" }))
    }
}

#[derive(Debug, Clone)]
struct Source {
    /// `\\.\DISPLAYn`
    name: String,
    attached: bool,
}

/// The driver's display sources that have a monitor plugged in, whether or
/// not Windows put it on the desktop.
fn driver_sources() -> Vec<Source> {
    let mut sources = Vec::new();
    for index in 0.. {
        let mut device = DISPLAY_DEVICEW { cb: size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
        // SAFETY: `device` is a correctly sized out struct.
        if !unsafe { EnumDisplayDevicesW(PCWSTR::null(), index, &mut device, 0) }.as_bool() {
            break;
        }
        if !amyuni::is_driver_source(&from_wide(&device.DeviceString)) {
            continue;
        }
        let attached = device.StateFlags.0 & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP.0 != 0;
        let mut monitor = DISPLAY_DEVICEW { cb: size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
        // SAFETY: DeviceName is NUL-terminated by Windows; `monitor` is a
        // correctly sized out struct.
        let has_monitor = unsafe { EnumDisplayDevicesW(PCWSTR(device.DeviceName.as_ptr()), 0, &mut monitor, 0) }.as_bool();
        if attached || has_monitor {
            sources.push(Source { name: from_wide(&device.DeviceName), attached });
        }
    }
    sources
}

fn source_names() -> Vec<String> { driver_sources().into_iter().map(|source| source.name).collect() }

/// The `HMONITOR` (as capture and input use it) showing `source`.
fn monitor_id(source: &str) -> Option<u32> {
    unsafe extern "system" fn each(monitor: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the Vec below, alive for the whole enumeration.
        let found = unsafe { &mut *(data.0 as *mut Vec<(HMONITOR, String)>) };
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        // SAFETY: MONITORINFOEXW starts with MONITORINFO and cbSize says
        // which one it is.
        if unsafe { GetMonitorInfoW(monitor, (&mut info as *mut MONITORINFOEXW).cast::<MONITORINFO>()) }.as_bool() {
            found.push((monitor, from_wide(&info.szDevice)));
        }
        true.into()
    }
    let mut found: Vec<(HMONITOR, String)> = Vec::new();
    // SAFETY: the callback only touches `found` during the call.
    unsafe { let _ = EnumDisplayMonitors(None, None, Some(each), LPARAM(&mut found as *mut Vec<(HMONITOR, String)> as isize)); }
    found.into_iter().find(|(_, name)| name.eq_ignore_ascii_case(source)).map(|(monitor, _)| monitor.0 as usize as u32)
}

/// Puts `source` in `mode`, and at `position` on the desktop if given.
fn set_mode(source: &str, (width, height): (u32, u32), position: Option<POINTL>) -> Result<(), String> {
    let name = wide(source);
    let mut mode = DEVMODEW { dmSize: size_of::<DEVMODEW>() as u16, dmPelsWidth: width, dmPelsHeight: height, ..Default::default() };
    mode.dmFields = DM_PELSWIDTH | DM_PELSHEIGHT;
    if let Some(position) = position {
        mode.Anonymous1.Anonymous2.dmPosition = position;
        mode.dmFields |= DM_POSITION;
    }
    // SAFETY: `name` is NUL-terminated and `mode` a correctly sized DEVMODEW.
    let staged = unsafe { ChangeDisplaySettingsExW(PCWSTR(name.as_ptr()), Some(&mode), None, CDS_UPDATEREGISTRY | CDS_NORESET, None) };
    if staged != DISP_CHANGE_SUCCESSFUL {
        return Err(format!("Windows refused the mode ({})", staged.0));
    }
    // SAFETY: applies the staged change; no pointers.
    let applied = unsafe { ChangeDisplaySettingsExW(PCWSTR::null(), None, None, CDS_TYPE(0), None) };
    if applied != DISP_CHANGE_SUCCESSFUL {
        return Err(format!("Windows could not apply the mode ({})", applied.0));
    }
    Ok(())
}

fn current_mode(source: &str) -> Option<(u32, u32)> {
    let name = wide(source);
    let mut mode = DEVMODEW { dmSize: size_of::<DEVMODEW>() as u16, ..Default::default() };
    // SAFETY: `name` is NUL-terminated and `mode` a correctly sized DEVMODEW.
    let ok = unsafe { EnumDisplaySettingsW(PCWSTR(name.as_ptr()), ENUM_CURRENT_SETTINGS, &mut mode) }.as_bool();
    (ok && mode.dmPelsWidth > 0 && mode.dmPelsHeight > 0).then_some((mode.dmPelsWidth, mode.dmPelsHeight))
}

fn right_of_desktop() -> POINTL {
    // SAFETY: plain metrics queries.
    let (left, width) = unsafe { (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_CXVIRTUALSCREEN)) };
    POINTL { x: left + width, y: 0 }
}

/// The modes the driver offers, from the registry; the presets if it
/// can't be read.
fn installed_modes() -> Vec<(u32, u32)> {
    for key in amyuni::MODE_KEYS {
        let modes: Vec<_> = (0..amyuni::MODE_SLOTS)
            .filter_map(|slot| registry_string(key, &slot.to_string()))
            .filter_map(|value| amyuni::parse_mode(&value))
            .collect();
        if !modes.is_empty() {
            return modes;
        }
    }
    amyuni::PRESET_MODES.to_vec()
}

fn registry_string(key: &str, value: &str) -> Option<String> {
    let (key, value) = (wide(key), wide(value));
    let mut buffer = [0u16; 64];
    let mut size = size_of_val(&buffer) as u32;
    // SAFETY: strings are NUL-terminated; `size` is the buffer's size in bytes.
    let status = unsafe {
        RegGetValueW(HKEY_LOCAL_MACHINE, PCWSTR(key.as_ptr()), PCWSTR(value.as_ptr()), RRF_RT_REG_SZ, None, Some(buffer.as_mut_ptr().cast()), Some(&mut size))
    };
    (status == ERROR_SUCCESS).then(|| from_wide(&buffer))
}

/// Installing and removing the driver. Everything here but [`installed`]
/// and [`run_elevated`] needs an elevated process: the app runs itself
/// elevated for it.
pub mod driver {
    use super::{amyuni, wide, DeviceSet};
    use std::path::Path;
    use windows::core::{GUID, PCWSTR};
    use windows::Win32::Devices::DeviceAndDriverInstallation::{
        SetupDiCallClassInstaller, SetupDiCreateDeviceInfoList, SetupDiCreateDeviceInfoW, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW,
        SetupDiGetDeviceRegistryPropertyW, SetupDiGetINFClassW, SetupDiSetClassInstallParamsW, SetupDiSetDeviceRegistryPropertyW,
        UpdateDriverForPlugAndPlayDevicesW, DICD_GENERATE_ID, DICS_FLAG_CONFIGSPECIFIC, DICS_PROPCHANGE, DIF_PROPERTYCHANGE,
        DIF_REGISTERDEVICE, DIF_REMOVE, DIGCF_ALLCLASSES, INSTALLFLAG_FORCE, SPDRP_HARDWAREID, SP_CLASSINSTALL_HEADER, SP_DEVINFO_DATA,
        SP_PROPCHANGE_PARAMS,
    };
    use windows::Win32::Foundation::{CloseHandle, HLOCAL, LocalFree, ERROR_CANCELLED, ERROR_SUCCESS, WAIT_OBJECT_0};
    use windows::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
    use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
    use windows::Win32::Storage::FileSystem::CreateDirectoryW;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
    };
    use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
    use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

    /// Whether the driver is installed and running.
    pub fn installed() -> bool { super::device_path().is_some() }

    /// Installs the driver package whose INF is `inf` (the catalog and
    /// `x64\usbmmIdd.dll` beside it), the way Amyuni's
    /// `deviceinstaller64 install usbmmidd.inf usbmmidd` does: a root
    /// device with the driver's hardware id, then Windows' own driver
    /// install, which checks the package's Microsoft signature. Then
    /// writes OpenDesk's mode list and restarts the device to load it.
    /// Returns whether Windows wants a restart. Needs administrator rights.
    pub fn install(inf: &Path) -> Result<bool, String> {
        let inf = wide(&inf.to_string_lossy());
        if device_count()? == 0 {
            create_device(&inf)?;
        }
        let hardware_id = wide(amyuni::HARDWARE_ID);
        let mut reboot = windows::core::BOOL(0);
        // SAFETY: both strings are NUL-terminated; `reboot` is an out BOOL.
        unsafe {
            UpdateDriverForPlugAndPlayDevicesW(None, PCWSTR(hardware_id.as_ptr()), PCWSTR(inf.as_ptr()), INSTALLFLAG_FORCE, Some(&mut reboot))
                .map_err(|error| format!("Windows did not install the driver: {error}"))?;
        }
        write_modes(&amyuni::PRESET_MODES, amyuni::DEFAULT_MODE)?;
        restart_devices();
        Ok(reboot.as_bool())
    }

    /// Removes every device of the driver's. The package stays in the
    /// driver store, as with Amyuni's `deviceinstaller64 remove`.
    pub fn uninstall() -> Result<usize, String> {
        each_device(true, |set, data| {
            // SAFETY: `data` belongs to `set`.
            unsafe { SetupDiCallClassInstaller(DIF_REMOVE, set, Some(data)) }.map_err(|error| format!("Windows did not remove the driver: {error}"))
        })
    }

    fn device_count() -> Result<usize, String> { each_device(true, |_, _| Ok(())) }

    /// Calls `act` on each device (present or not, unless `all` is false)
    /// whose hardware ids include the driver's. Returns how many matched.
    fn each_device(
        all: bool,
        mut act: impl FnMut(windows::Win32::Devices::DeviceAndDriverInstallation::HDEVINFO, &SP_DEVINFO_DATA) -> Result<(), String>,
    ) -> Result<usize, String> {
        use windows::Win32::Devices::DeviceAndDriverInstallation::DIGCF_PRESENT;
        let flags = if all { DIGCF_ALLCLASSES } else { DIGCF_ALLCLASSES | DIGCF_PRESENT };
        // SAFETY: standard SetupAPI enumeration over a set this owns.
        let set = DeviceSet(unsafe { SetupDiGetClassDevsW(None, PCWSTR::null(), None, flags) }.map_err(|error| error.to_string())?);
        let mut matched = 0;
        for index in 0.. {
            let mut data = SP_DEVINFO_DATA { cbSize: size_of::<SP_DEVINFO_DATA>() as u32, ..Default::default() };
            // SAFETY: `data` is a correctly sized out struct.
            if unsafe { SetupDiEnumDeviceInfo(set.0, index, &mut data) }.is_err() {
                break;
            }
            let mut ids = [0u8; 2048];
            // SAFETY: `ids` is the out buffer; a failure (no hardware ids,
            // or too many) just skips the device.
            let read = unsafe { SetupDiGetDeviceRegistryPropertyW(set.0, &data, SPDRP_HARDWAREID, None, Some(&mut ids), None) };
            if read.is_err() || !amyuni::has_hardware_id(&ids) {
                continue;
            }
            act(set.0, &data)?;
            matched += 1;
        }
        Ok(matched)
    }

    /// A root-enumerated device with the driver's hardware id (devcon's
    /// `install` without the driver part).
    fn create_device(inf: &[u16]) -> Result<(), String> {
        let failed = |step: &str, error: windows::core::Error| format!("{step}: {error}");
        // SAFETY: NUL-terminated strings, correctly sized out structs, and
        // a device set this owns.
        unsafe {
            let mut class = GUID::zeroed();
            let mut class_name = [0u16; 32];
            SetupDiGetINFClassW(PCWSTR(inf.as_ptr()), &mut class, &mut class_name, None).map_err(|error| failed("reading the driver's INF", error))?;
            let set = DeviceSet(SetupDiCreateDeviceInfoList(Some(&class), None).map_err(|error| failed("creating the device", error))?);
            let mut data = SP_DEVINFO_DATA { cbSize: size_of::<SP_DEVINFO_DATA>() as u32, ..Default::default() };
            SetupDiCreateDeviceInfoW(set.0, PCWSTR(class_name.as_ptr()), &class, PCWSTR::null(), None, DICD_GENERATE_ID, Some(&mut data))
                .map_err(|error| failed("creating the device", error))?;
            // REG_MULTI_SZ: the id, then an empty string.
            let ids: Vec<u8> = amyuni::HARDWARE_ID.encode_utf16().chain([0, 0]).flat_map(u16::to_le_bytes).collect();
            SetupDiSetDeviceRegistryPropertyW(set.0, &mut data, SPDRP_HARDWAREID, Some(&ids)).map_err(|error| failed("naming the device", error))?;
            SetupDiCallClassInstaller(DIF_REGISTERDEVICE, set.0, Some(&data)).map_err(|error| failed("registering the device", error))?;
        }
        Ok(())
    }

    /// Restarts the driver's devices so it reads the new mode list.
    fn restart_devices() {
        let restarted = each_device(false, |set, data| {
            let params = SP_PROPCHANGE_PARAMS {
                ClassInstallHeader: SP_CLASSINSTALL_HEADER { cbSize: size_of::<SP_CLASSINSTALL_HEADER>() as u32, InstallFunction: DIF_PROPERTYCHANGE },
                StateChange: DICS_PROPCHANGE,
                Scope: DICS_FLAG_CONFIGSPECIFIC,
                HwProfile: 0,
            };
            // SAFETY: `params` starts with its header and is passed with its full size.
            unsafe {
                SetupDiSetClassInstallParamsW(set, Some(data), Some(&params.ClassInstallHeader), size_of::<SP_PROPCHANGE_PARAMS>() as u32)
                    .and_then(|()| SetupDiCallClassInstaller(DIF_PROPERTYCHANGE, set, Some(data)))
                    .map_err(|error| error.to_string())
            }
        });
        if let Err(error) = restarted {
            log::warn!("could not restart the virtual display driver: {error}");
        }
    }

    /// Writes the mode list wherever the driver's install put its own (see
    /// [`amyuni::MODE_KEYS`]); the second key if neither exists yet.
    pub fn write_modes(modes: &[(u32, u32)], default: (u32, u32)) -> Result<(), String> {
        let values = amyuni::mode_values(modes, default);
        let mut wrote = false;
        for key in amyuni::MODE_KEYS {
            if let Some(handle) = open_key(key, false) {
                let result = set_values(handle, &values);
                // SAFETY: closes the key opened above.
                unsafe { let _ = RegCloseKey(handle); }
                result?;
                wrote = true;
            }
        }
        if !wrote {
            let handle = open_key(amyuni::MODE_KEYS[1], true).ok_or("could not create the driver's mode key")?;
            let result = set_values(handle, &values);
            // SAFETY: closes the key created above.
            unsafe { let _ = RegCloseKey(handle); }
            result?;
        }
        Ok(())
    }

    fn open_key(key: &str, create: bool) -> Option<HKEY> {
        let key = wide(key);
        let mut handle = HKEY::default();
        // SAFETY: NUL-terminated name; `handle` is an out HKEY.
        let status = unsafe {
            if create {
                RegCreateKeyExW(HKEY_LOCAL_MACHINE, PCWSTR(key.as_ptr()), None, PCWSTR::null(), REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, None, &mut handle, None)
            } else {
                RegOpenKeyExW(HKEY_LOCAL_MACHINE, PCWSTR(key.as_ptr()), None, KEY_SET_VALUE, &mut handle)
            }
        };
        (status == ERROR_SUCCESS).then_some(handle)
    }

    fn set_values(handle: HKEY, values: &[(String, String)]) -> Result<(), String> {
        for (name, data) in values {
            let name = wide(name);
            let data: Vec<u8> = wide(data).into_iter().flat_map(u16::to_le_bytes).collect();
            // SAFETY: open key, NUL-terminated name, REG_SZ data with its NUL.
            let status = unsafe { RegSetValueExW(handle, PCWSTR(name.as_ptr()), None, REG_SZ, Some(&data)) };
            if status != ERROR_SUCCESS {
                return Err(format!("could not write the driver's modes ({})", status.0));
            }
        }
        Ok(())
    }

    /// Creates `path` readable and writable only by SYSTEM and
    /// Administrators, inheriting nothing, so no unelevated program can
    /// add or swap files in it.
    pub fn create_private_dir(path: &Path) -> Result<(), String> {
        let sddl = wide("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)");
        let name = wide(&path.to_string_lossy());
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // SAFETY: NUL-terminated SDDL; the descriptor is freed below.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(sddl.as_ptr()), 1, &mut descriptor, None).map_err(|error| error.to_string())?;
            let attributes = SECURITY_ATTRIBUTES { nLength: size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: descriptor.0, bInheritHandle: false.into() };
            let created = CreateDirectoryW(PCWSTR(name.as_ptr()), Some(&attributes));
            let _ = LocalFree(Some(HLOCAL(descriptor.0)));
            created.map_err(|error| format!("could not create {}: {error}", path.display()))
        }
    }

    /// Why [`run_elevated`] failed.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum ElevationError {
        /// The user said no to Windows' administrator prompt.
        Declined,
        Failed(String),
    }

    /// Runs `program` with `parameters` as administrator (Windows asks the
    /// user) and waits for it. Returns its exit code.
    pub fn run_elevated(program: &Path, parameters: &str) -> Result<u32, ElevationError> {
        let (verb, file, parameters) = (wide("runas"), wide(&program.to_string_lossy()), wide(parameters));
        let mut info = SHELLEXECUTEINFOW {
            cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
            lpVerb: PCWSTR(verb.as_ptr()),
            lpFile: PCWSTR(file.as_ptr()),
            lpParameters: PCWSTR(parameters.as_ptr()),
            nShow: SW_HIDE.0,
            ..Default::default()
        };
        // SAFETY: every string outlives the call; the process handle is
        // closed once.
        unsafe {
            if let Err(error) = ShellExecuteExW(&mut info) {
                if error.code() == ERROR_CANCELLED.to_hresult() {
                    return Err(ElevationError::Declined);
                }
                return Err(ElevationError::Failed(error.to_string()));
            }
            if info.hProcess.is_invalid() {
                return Err(ElevationError::Failed("Windows started no process".into()));
            }
            let waited = WaitForSingleObject(info.hProcess, INFINITE);
            let mut code = 1u32;
            let read = GetExitCodeProcess(info.hProcess, &mut code);
            let _ = CloseHandle(info.hProcess);
            if waited != WAIT_OBJECT_0 || read.is_err() {
                return Err(ElevationError::Failed("lost track of the installer".into()));
            }
            Ok(code)
        }
    }
}
