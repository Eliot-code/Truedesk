pub fn screen_recording() -> bool {
    scap::has_permission()
}

pub fn accessibility() -> bool {
    host_core::inject::EnigoInjector::new().is_some()
}

pub fn settings_url(which: &str) -> &'static str {
    match which {
        "screen" => "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
        "accessibility" => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        }
        _ => "",
    }
}

pub fn open_settings(which: &str) -> Result<(), String> {
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

#[cfg(test)]
mod tests {
    use super::settings_url;

    #[test]
    fn settings_urls_are_mapped() {
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
}
