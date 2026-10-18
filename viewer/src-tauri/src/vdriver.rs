//! Windows' virtual display driver: Amyuni's usbmmidd_v2, which the host
//! user installs from Home. Nothing here runs unless they click
//! "Install virtual display driver" (or "Uninstall").
//!
//! Install: download the package from Amyuni over HTTPS, check it against
//! a pinned SHA-256, then run this app again as administrator (Windows
//! asks the user) with the downloaded file's path. That elevated copy
//! reads the file into memory, checks the hash again (the file sat in the
//! user's temp folder meanwhile), unpacks the three files it needs into a
//! folder only administrators can write to, and installs the driver
//! through Windows' own driver installer, which also checks the package's
//! Microsoft signature. Amyuni's `deviceinstaller64.exe` is never run.

// Most of this only runs on 64-bit Windows; tests cover it everywhere.
#![cfg_attr(not(all(windows, target_arch = "x86_64")), allow(dead_code))]

use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::path::PathBuf;

/// Amyuni's download (linked from the package's own instructions).
pub(crate) const PACKAGE_URL: &str = "https://www.amyuni.com/downloads/usbmmidd_v2.zip";
/// SHA-256 of that file: 199,309 bytes, last modified 2021-09-09, driver
/// version 2.0.0.1. Checked on 2026-10-13; the copy RustDesk mirrors
/// (rustdesk-org/rdev, release `usbmmidd_v2`) is byte-identical.
pub(crate) const PACKAGE_SHA256: &str = "629b51e9944762bae73948171c65d09a79595cf4c771a82ebc003fbba5b24f51";
/// Well above the package's size; anything bigger is not it.
const PACKAGE_MAX: usize = 1 << 20;
/// What the install uses from the package: (path in the zip, path in the
/// install folder). The INF looks for its DLL under `x64\`.
pub(crate) const PACKAGE_FILES: [(&str, &str); 3] = [
    ("usbmmidd_v2/usbmmIdd.inf", "usbmmIdd.inf"),
    ("usbmmidd_v2/usbmmidd.cat", "usbmmidd.cat"),
    ("usbmmidd_v2/x64/usbmmIdd.dll", "x64/usbmmIdd.dll"),
];

/// The package's License.txt, word for word. Home shows it before the
/// install; the README carries it too.
pub(crate) const LICENSE: &str = "Copyright 2014-2021 Amyuni Technologies Inc.
https://www.amyuni.com

This software is provided 'as-is', without any express or implied warranty. In no event will the authors be held liable for any damages arising from the use of this software.
By using this software, you accept to be prompted with an advertisement page which is not always under the control of the author. A fully commercial version is available for users who do not wisth to see those advertisements.

Permission is granted to anyone to use this software for any purpose subject to the following restrictions:

1. The origin of this software must not be misrepresented; you must not claim that you wrote the original software.
   If you use this software in a product, an acknowledgment in the product documentation would be required.
2. Altered versions must be plainly marked as such, and must not be misrepresented as being the original software.
3. This notice may not be removed or altered from any distribution.";

/// Starts the elevated helper: `<exe> HELPER_ARG install <zip>` or
/// `<exe> HELPER_ARG uninstall`.
pub(crate) const HELPER_ARG: &str = "--truedesk-virtual-display-driver";

/// Helper exit codes.
mod exit {
    pub const OK: i32 = 0;
    /// ERROR_SUCCESS_REBOOT_REQUIRED
    pub const OK_RESTART: i32 = 3010;
    pub const BAD_PACKAGE: i32 = 20;
    pub const NO_FOLDER: i32 = 21;
    pub const INSTALL_FAILED: i32 = 22;
    pub const UNINSTALL_FAILED: i32 = 23;
    pub const USAGE: i32 = 24;
}

/// What the elevated helper was asked to do.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum HelperTask {
    Install(PathBuf),
    Uninstall,
}

/// The helper task in this process's arguments (without the program
/// name), if it was started as the helper. `Err` is a usage error.
pub(crate) fn helper_task(args: &[OsString]) -> Option<Result<HelperTask, ()>> {
    let (flag, rest) = args.split_first()?;
    if flag != HELPER_ARG {
        return None;
    }
    Some(match rest {
        [task, zip] if task == "install" => Ok(HelperTask::Install(PathBuf::from(zip))),
        [task] if task == "uninstall" => Ok(HelperTask::Uninstall),
        _ => Err(()),
    })
}

/// Whether `bytes` is exactly the pinned package.
pub(crate) fn verify(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > PACKAGE_MAX {
        return Err("the download is far bigger than the driver package".into());
    }
    let digest = Sha256::digest(bytes);
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    if hex != PACKAGE_SHA256 {
        return Err(format!("the download doesn't match the known driver package (SHA-256 {hex})"));
    }
    Ok(())
}

/// Unpacks [`PACKAGE_FILES`] from the verified package into `dir`.
/// Returns the INF's path.
#[cfg(any(windows, test))]
pub(crate) fn unpack(bytes: &[u8], dir: &std::path::Path) -> Result<PathBuf, String> {
    use std::io::Read;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|error| format!("unreadable package: {error}"))?;
    for (inside, outside) in PACKAGE_FILES {
        let mut entry = archive.by_name(inside).map_err(|error| format!("{inside}: {error}"))?;
        let mut contents = Vec::new();
        entry.by_ref().take(PACKAGE_MAX as u64).read_to_end(&mut contents).map_err(|error| format!("{inside}: {error}"))?;
        let path = dir.join(outside);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
        }
        std::fs::write(&path, contents).map_err(|error| format!("{}: {error}", path.display()))?;
    }
    Ok(dir.join(PACKAGE_FILES[0].1))
}

/// What the helper's exit code means for the user: `Ok(true)` when
/// Windows wants a restart.
pub(crate) fn helper_outcome(code: u32) -> Result<bool, String> {
    match i32::try_from(code).unwrap_or(-1) {
        exit::OK => Ok(false),
        exit::OK_RESTART => Ok(true),
        exit::BAD_PACKAGE => Err("The driver package didn't check out, so nothing was installed.".into()),
        exit::NO_FOLDER => Err("Couldn't prepare a folder for the driver, so nothing was installed.".into()),
        exit::INSTALL_FAILED => Err("Windows didn't install the driver. See Device Manager → Display adapters for details.".into()),
        exit::UNINSTALL_FAILED => Err("Windows didn't remove the driver. You can uninstall \"USB Mobile Monitor Virtual Display\" in Device Manager.".into()),
        other => Err(format!("The driver installer stopped unexpectedly (code {other}).")),
    }
}

/// The driver's state on this computer, for Home.
#[derive(serde::Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DriverStatus {
    /// This platform installs virtual displays through the driver
    /// (64-bit Windows).
    pub(crate) applies: bool,
    pub(crate) installed: bool,
    /// Amyuni's licence, which Home shows before installing.
    pub(crate) license: &'static str,
}

pub(crate) fn status() -> DriverStatus {
    #[cfg(all(windows, target_arch = "x86_64"))]
    return DriverStatus { applies: true, installed: host_core::vdisplay::win32::driver::installed(), license: LICENSE };
    #[cfg(not(all(windows, target_arch = "x86_64")))]
    return DriverStatus { applies: false, installed: false, license: LICENSE };
}

#[tauri::command]
pub(crate) fn virtual_display_driver_status() -> DriverStatus { status() }

/// Downloads, checks and installs the driver. Resolves once Windows is
/// done; `Ok` carries the new status, and `restart` when Windows wants one.
#[tauri::command]
pub(crate) async fn install_virtual_display_driver() -> Result<InstallResult, String> {
    #[cfg(all(windows, target_arch = "x86_64"))]
    {
        let bytes = platform::download().await?;
        verify(&bytes)?;
        tauri::async_runtime::spawn_blocking(move || platform::install(&bytes)).await.map_err(|error| error.to_string())?
    }
    #[cfg(not(all(windows, target_arch = "x86_64")))]
    Err("The virtual display driver is for 64-bit Windows.".into())
}

#[tauri::command]
pub(crate) async fn uninstall_virtual_display_driver() -> Result<InstallResult, String> {
    #[cfg(all(windows, target_arch = "x86_64"))]
    {
        tauri::async_runtime::spawn_blocking(platform::uninstall).await.map_err(|error| error.to_string())?
    }
    #[cfg(not(all(windows, target_arch = "x86_64")))]
    Err("The virtual display driver is for 64-bit Windows.".into())
}

#[derive(serde::Serialize, Clone, Copy, Debug)]
pub(crate) struct InstallResult {
    pub(crate) status: DriverStatus,
    /// Windows asked for a restart to finish.
    pub(crate) restart: bool,
}

/// Runs the elevated half if this process was started for it, and
/// returns its exit code; `None` for a normal launch.
pub(crate) fn run_helper(args: &[OsString]) -> Option<i32> {
    let task = helper_task(args)?;
    #[cfg(all(windows, target_arch = "x86_64"))]
    return Some(match task {
        Ok(task) => platform::helper(task),
        Err(()) => exit::USAGE,
    });
    #[cfg(not(all(windows, target_arch = "x86_64")))]
    {
        let _ = task;
        Some(exit::USAGE)
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
mod platform {
    use super::{exit, helper_outcome, status, unpack, verify, HelperTask, InstallResult, HELPER_ARG, PACKAGE_MAX, PACKAGE_URL};
    use host_core::vdisplay::win32::driver::{self, ElevationError};
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    pub(super) async fn download() -> Result<Vec<u8>, String> {
        host_core::relay_client::install_crypto_provider();
        let failed = |error: reqwest::Error| format!("Couldn't download the driver from amyuni.com: {error}");
        let client = reqwest::Client::builder().https_only(true).timeout(Duration::from_secs(60)).build().map_err(failed)?;
        let response = client.get(PACKAGE_URL).send().await.and_then(reqwest::Response::error_for_status).map_err(failed)?;
        if response.content_length().is_some_and(|length| length > PACKAGE_MAX as u64) {
            return Err("The download is far bigger than the driver package.".into());
        }
        Ok(response.bytes().await.map_err(failed)?.to_vec())
    }

    fn unique(prefix: &str) -> String {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_nanos());
        format!("{prefix}-{}-{nanos}", std::process::id())
    }

    fn elevate(parameters: &str) -> Result<InstallResult, String> {
        let exe = std::env::current_exe().map_err(|error| error.to_string())?;
        let code = driver::run_elevated(&exe, parameters).map_err(|error| match error {
            ElevationError::Declined => "Nothing was changed: Windows' administrator prompt was declined.".to_owned(),
            ElevationError::Failed(reason) => format!("Couldn't start the installer as administrator: {reason}"),
        })?;
        let restart = helper_outcome(code)?;
        // The device can take a moment to start (or go) after the helper exits.
        let installing = parameters.contains(" install ");
        for _ in 0..20 {
            if status().installed == installing {
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        Ok(InstallResult { status: status(), restart })
    }

    pub(super) fn install(bytes: &[u8]) -> Result<InstallResult, String> {
        // Handed to the elevated copy by path; it checks the hash itself.
        let zip = std::env::temp_dir().join(format!("{}.zip", unique("truedesk-usbmmidd")));
        std::fs::write(&zip, bytes).map_err(|error| format!("Couldn't save the download: {error}"))?;
        let result = elevate(&format!("{HELPER_ARG} install \"{}\"", zip.display()));
        let _ = std::fs::remove_file(&zip);
        result
    }

    pub(super) fn uninstall() -> Result<InstallResult, String> {
        host_core::vdisplay::remove_owned();
        elevate(&format!("{HELPER_ARG} uninstall"))
    }

    /// The elevated half.
    pub(super) fn helper(task: HelperTask) -> i32 {
        match task {
            HelperTask::Install(zip) => helper_install(&zip),
            HelperTask::Uninstall => match driver::uninstall() {
                Ok(_) => exit::OK,
                Err(_) => exit::UNINSTALL_FAILED,
            },
        }
    }

    fn helper_install(zip: &Path) -> i32 {
        let Ok(bytes) = std::fs::read(zip) else { return exit::BAD_PACKAGE };
        if verify(&bytes).is_err() {
            return exit::BAD_PACKAGE;
        }
        // Under %SystemRoot%\Temp, created fresh with an administrators-only
        // ACL: CreateDirectoryW fails if anything got there first.
        let root = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        let dir = root.join("Temp").join(unique("truedesk-usbmmidd"));
        if driver::create_private_dir(&dir).is_err() {
            return exit::NO_FOLDER;
        }
        let code = match unpack(&bytes, &dir) {
            Err(_) => exit::BAD_PACKAGE,
            Ok(inf) => match driver::install(&inf) {
                Ok(false) => exit::OK,
                Ok(true) => exit::OK_RESTART,
                Err(_) => exit::INSTALL_FAILED,
            },
        };
        let _ = std::fs::remove_dir_all(&dir);
        code
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn package(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, contents) in files {
            out.start_file(*name, zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated)).unwrap();
            out.write_all(contents).unwrap();
        }
        out.finish().unwrap().into_inner()
    }

    fn args(list: &[&str]) -> Vec<OsString> { list.iter().map(OsString::from).collect() }

    #[test]
    fn only_the_pinned_package_passes() {
        assert!(verify(b"not the driver").unwrap_err().contains("doesn't match"));
        assert!(verify(&vec![0; PACKAGE_MAX + 1]).unwrap_err().contains("bigger"));
        // The pin is a full lowercase SHA-256.
        assert_eq!(PACKAGE_SHA256.len(), 64);
        assert!(PACKAGE_SHA256.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert!(PACKAGE_URL.starts_with("https://www.amyuni.com/"));
    }

    #[test]
    fn unpacking_takes_only_the_driver_files() {
        let zip = package(&[
            ("usbmmidd_v2/deviceinstaller64.exe", b"MZ"),
            ("usbmmidd_v2/usbmmIdd.inf", b"[Version]"),
            ("usbmmidd_v2/usbmmidd.cat", b"catalog"),
            ("usbmmidd_v2/x64/usbmmIdd.dll", b"dll"),
            ("usbmmidd_v2/Win32/usbmmIdd.dll", b"dll32"),
        ]);
        let dir = std::env::temp_dir().join(format!("truedesk-vdriver-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let inf = unpack(&zip, &dir).unwrap();
        assert_eq!(inf, dir.join("usbmmIdd.inf"));
        assert_eq!(std::fs::read(dir.join("x64").join("usbmmIdd.dll")).unwrap(), b"dll");
        assert_eq!(std::fs::read(dir.join("usbmmidd.cat")).unwrap(), b"catalog");
        assert!(!dir.join("deviceinstaller64.exe").exists() && !dir.join("Win32").exists());
        let _ = std::fs::remove_dir_all(&dir);

        let missing = package(&[("usbmmidd_v2/usbmmIdd.inf", b"[Version]")]);
        assert!(unpack(&missing, &std::env::temp_dir()).unwrap_err().contains("usbmmidd.cat"));
    }

    /// Against the real download: `TRUEDESK_USBMMIDD_ZIP=path/to/usbmmidd_v2.zip
    /// cargo test real_package -- --ignored`
    #[test]
    #[ignore = "needs the downloaded package"]
    fn real_package_verifies_and_unpacks() {
        let path = std::env::var_os("TRUEDESK_USBMMIDD_ZIP").expect("set TRUEDESK_USBMMIDD_ZIP");
        let bytes = std::fs::read(path).unwrap();
        verify(&bytes).unwrap();
        let dir = std::env::temp_dir().join(format!("truedesk-vdriver-real-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let inf = unpack(&bytes, &dir).unwrap();
        // The INF is UTF-16 and names the catalog and hardware id.
        let units: Vec<u16> = std::fs::read(&inf).unwrap().as_chunks::<2>().0.iter().map(|&pair| u16::from_le_bytes(pair)).collect();
        let text = String::from_utf16_lossy(&units);
        assert!(text.contains("CatalogFile=usbmmIdd.cat") && text.contains("MyDevice_Install, usbmmIdd"), "{text}");
        assert_eq!(std::fs::metadata(dir.join("x64").join("usbmmIdd.dll")).unwrap().len(), 71_616);
        let _ = std::fs::remove_dir_all(&dir);
        // The licence Home shows is the package's own.
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        let mut license = String::new();
        std::io::Read::read_to_string(&mut archive.by_name("usbmmidd_v2/License.txt").unwrap(), &mut license).unwrap();
        assert_eq!(license.replace("\r\n", "\n").trim_end(), LICENSE);
    }

    #[test]
    fn the_helper_is_only_started_by_its_own_flag() {
        assert_eq!(helper_task(&args(&[])), None);
        assert_eq!(helper_task(&args(&["--background"])), None);
        assert_eq!(helper_task(&args(&[HELPER_ARG, "install", r"C:\Temp\a b.zip"])), Some(Ok(HelperTask::Install(PathBuf::from(r"C:\Temp\a b.zip")))));
        assert_eq!(helper_task(&args(&[HELPER_ARG, "uninstall"])), Some(Ok(HelperTask::Uninstall)));
        assert_eq!(helper_task(&args(&[HELPER_ARG, "install"])), Some(Err(())));
        assert_eq!(helper_task(&args(&[HELPER_ARG, "run", "x"])), Some(Err(())));
    }

    #[test]
    fn helper_exit_codes_read_as_outcomes() {
        assert_eq!(helper_outcome(0), Ok(false));
        assert_eq!(helper_outcome(3010), Ok(true));
        assert!(helper_outcome(20).unwrap_err().contains("didn't check out"));
        assert!(helper_outcome(23).unwrap_err().contains("Device Manager"));
        assert!(helper_outcome(u32::MAX).unwrap_err().contains("unexpectedly"));
    }

    #[test]
    fn the_license_shown_is_amyunis() {
        assert!(LICENSE.starts_with("Copyright 2014-2021 Amyuni Technologies Inc."));
        assert!(LICENSE.contains("advertisement page") && LICENSE.ends_with("from any distribution."));
    }

    #[test]
    fn other_platforms_have_nothing_to_install() {
        if !cfg!(all(windows, target_arch = "x86_64")) {
            assert_eq!(status(), DriverStatus { applies: false, installed: false, license: LICENSE });
            assert_eq!(run_helper(&args(&[HELPER_ARG, "uninstall"])), Some(24));
        }
        assert_eq!(run_helper(&args(&["--background"])), None);
    }
}
