pub fn screen_recording() -> bool {
    #[cfg(target_os = "macos")]
    {
        scap::has_permission()
    }

    #[cfg(not(target_os = "macos"))]
    {
        // Windows Graphics Capture and the Linux portal request access as
        // part of capturer startup; there is no macOS-style preflight check.
        true
    }
}

pub fn accessibility() -> bool {
    #[cfg(target_os = "macos")]
    {
        host_core::inject::EnigoInjector::new().is_some()
    }

    #[cfg(not(target_os = "macos"))]
    {
        // Enigo uses SendInput on Windows and XTest/uinput on Linux. Failure
        // is reported when an event is injected, not through a macOS-style
        // accessibility database.
        true
    }
}

/// The System Settings pane for a macOS privacy permission.
#[cfg(target_os = "macos")]
fn settings_url(which: &str) -> &'static str {
    match which {
        "screen" => "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
        "accessibility" => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        }
        _ => "",
    }
}

pub fn open_settings(which: &str) -> Result<(), String> {
    #[cfg(not(target_os = "macos"))]
    {
        // Nothing to open: other platforms ask while capture or input starts.
        match which {
            "screen" | "accessibility" => Ok(()),
            _ => Err(format!("unknown permission: {which}")),
        }
    }

    #[cfg(target_os = "macos")]
    {
    let url = settings_url(which);
    if url.is_empty() {
        return Err(format!("unknown permission: {which}"));
    }
    std::process::Command::new("open")
        .arg(url)
        .status()
        .map_err(|error| format!("could not open System Settings: {error}"))
        .and_then(|status| {
            if status.success() {
                Ok(())
            } else {
                Err("System Settings refused to open".into())
            }
        })
    }
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "macos")]
    #[test]
    fn settings_urls_are_mapped() {
        use super::settings_url;
        assert_eq!(
            settings_url("screen"),
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
        );
        assert_eq!(
            settings_url("accessibility"),
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        );
        assert_eq!(settings_url("other"), "");
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn other_platforms_have_nothing_to_open() {
        assert_eq!(super::open_settings("screen"), Ok(()));
        assert_eq!(super::open_settings("accessibility"), Ok(()));
        assert!(super::open_settings("other").is_err());
    }
}
