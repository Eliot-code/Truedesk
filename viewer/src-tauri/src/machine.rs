//! Which computer this is, so a config copied to another one (a cloned disk,
//! a restored backup) can be told apart from the original.

use sha2::{Digest, Sha256};

/// A hash of the OS machine identifier (IOPlatformUUID on macOS, MachineGuid
/// on Windows, /etc/machine-id on Linux), or `None` when it can't be read.
/// Only the hash is stored, never the identifier itself.
pub(crate) fn fingerprint() -> Option<String> {
    hash(&os_machine_id()?)
}

fn hash(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let digest = Sha256::digest(format!("opendesk machine {raw}").as_bytes());
    Some(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(target_os = "macos")]
fn os_machine_id() -> Option<String> {
    let output = std::process::Command::new("/usr/sbin/ioreg")
        .args(["-rd1", "-c", "IOPlatformExpertDevice"])
        .output()
        .ok()?;
    platform_uuid(&String::from_utf8_lossy(&output.stdout))
}

/// Pulls `IOPlatformUUID` out of `ioreg -rd1 -c IOPlatformExpertDevice`.
#[cfg(any(target_os = "macos", test))]
fn platform_uuid(ioreg: &str) -> Option<String> {
    ioreg.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        (key.trim() == "\"IOPlatformUUID\"").then(|| value.trim().trim_matches('"').to_owned())
    })
}

#[cfg(windows)]
fn os_machine_id() -> Option<String> {
    windows_registry::LOCAL_MACHINE
        .open(r"SOFTWARE\Microsoft\Cryptography")
        .and_then(|key| key.get_string("MachineGuid"))
        .ok()
}

#[cfg(target_os = "linux")]
fn os_machine_id() -> Option<String> {
    ["/etc/machine-id", "/var/lib/dbus/machine-id"]
        .iter()
        .find_map(|path| std::fs::read_to_string(path).ok().filter(|id| !id.trim().is_empty()))
}

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
fn os_machine_id() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_platform_uuid_from_ioreg() {
        let ioreg = r#"+-o J316sAP  <class IOPlatformExpertDevice, id 0x100000213, registered>
    {
      "IOPlatformSerialNumber" = "C02XXXXXXX"
      "IOPlatformUUID" = "4C4C4544-0035-4810-8031-B8C04F4E4B32"
      "IOPolledInterface" = "AppleARMWatchdogTimerHibernateHandler is not serializable"
    }"#;
        assert_eq!(platform_uuid(ioreg).as_deref(), Some("4C4C4544-0035-4810-8031-B8C04F4E4B32"));
        assert_eq!(platform_uuid("nothing here"), None);
    }

    #[test]
    fn only_a_hash_of_the_id_is_kept() {
        let hashed = hash("  4c4c4544-0035  \n").unwrap();
        assert_eq!(hashed.len(), 64);
        assert!(!hashed.contains("4c4c4544"));
        assert_eq!(hash("4c4c4544-0035"), Some(hashed));
        assert_eq!(hash(" \n"), None);
    }

    #[test]
    fn this_machine_has_a_stable_fingerprint() {
        // Some containers have no machine ID; where there is one it must not
        // change between reads.
        let first = fingerprint();
        assert_eq!(fingerprint(), first);
    }
}
