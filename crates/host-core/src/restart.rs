//! Restarting the host computer so a viewer can reconnect after it comes back.
//!
//! Only Windows is supported for now: a standard user can reboot with
//! `shutdown /r` without a prompt. macOS and Linux need elevated rights
//! (polkit/admin), so the feature is advertised off there until a privileged
//! helper exists.

/// Whether this platform can restart without an admin prompt.
pub fn available() -> bool {
    cfg!(windows)
}

/// Restart the computer after a few seconds, leaving time for the
/// `Restarting` control frame to reach the viewer. Best-effort: returns an
/// error if the restart could not be started.
pub fn restart() -> Result<(), String> {
    #[cfg(windows)]
    {
        std::process::Command::new("shutdown")
            .args(["/r", "/t", "3", "/c", "OpenDesk remote restart"])
            .spawn()
            .map(|_child| ())
            .map_err(|error| error.to_string())
    }
    #[cfg(not(windows))]
    {
        Err("restarting the host is not supported on this platform".into())
    }
}
