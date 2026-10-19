const COMMANDS: &[&str] = &[
    "host_state",
    "open_accessibility_settings",
    "start_hosting",
    "stop_hosting",
    "answer_request",
    "poll_event",
];

fn main() {
    tauri_plugin::Builder::new(COMMANDS).android_path("android").build();

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let mobile = target_os == "ios" || target_os == "android";
    alias("mobile", mobile);
    alias("desktop", !mobile);
}

/// Creates a cfg alias if `enabled`. `alias` must be snake case.
fn alias(alias: &str, enabled: bool) {
    println!("cargo:rustc-check-cfg=cfg({alias})");
    if enabled {
        println!("cargo:rustc-cfg={alias}");
    }
}
