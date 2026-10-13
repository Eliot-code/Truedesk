//! Starting at sign-in: the `--background` launch, the tray icon, and keeping
//! a reachable host running when its window is closed.
//!
//! Everything here runs as the signed-in user. Nothing starts before login.

use super::*;
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
};
use tauri_plugin_autostart::ManagerExt;

/// Passed by the sign-in entry the autostart plugin registers.
pub(crate) const BACKGROUND_ARG: &str = "--background";

const TRAY_ID: &str = "main";

/// Whether this launch came from the sign-in entry. `args` excludes the
/// program name.
pub(crate) fn launched_in_background<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    args.into_iter().any(|arg| arg.as_ref() == BACKGROUND_ARG)
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Startup {
    /// A normal launch: show the window; Home starts hosting as usual.
    ShowWindow,
    /// Stay in the tray and start hosting right away.
    HostHidden,
}

/// A background launch only hides the window when someone can actually
/// connect unattended. Otherwise a hidden host would just sit there waiting
/// for an approval nobody can give, so open the window like a normal launch.
pub(crate) fn startup_mode(background: bool, unattended_ready: bool) -> Startup {
    if background && unattended_ready {
        Startup::HostHidden
    } else {
        Startup::ShowWindow
    }
}

/// Closing the window hides it to the tray instead of quitting only when the
/// user asked to start at sign-in and the host is reachable unattended.
/// Everyone else keeps the old behaviour: closing the window quits.
pub(crate) fn keep_running_on_close(autostart_enabled: bool, unattended_ready: bool) -> bool {
    autostart_enabled && unattended_ready
}

/// Tray label for the hosting item, given the current host status.
pub(crate) fn hosting_item_label(host_status: &str) -> &'static str {
    if host_status == "offline" { "Start hosting" } else { "Stop hosting" }
}

/// Needs the secrets to actually be readable: the config alone can say "set"
/// after the keyring was wiped, locked, or left behind on another machine.
fn unattended_ready(app: &AppHandle) -> bool {
    let state = app.state::<AppState>();
    let check = unattended::check_config(&state.lock().config.config);
    if let Some(problem) = &check.problem {
        log::warn!("unattended access is not ready: {problem}");
    }
    check.ready()
}

/// Called from `setup` once `AppState` is managed. The main window is
/// created hidden (tauri.conf.json) so a background launch never flashes it.
pub(crate) fn start(app: &AppHandle, background: bool) {
    // Only a background launch needs the keychain lookup.
    let ready = background && unattended_ready(app);
    match startup_mode(background, ready) {
        Startup::ShowWindow => show_main_window(app),
        Startup::HostHidden => {
            log::info!("started at sign-in; hosting in the background");
            if let Err(error) = ensure_tray(app) {
                // Without a tray there is no way back to a hidden window.
                log::warn!("could not create the tray icon: {error}");
                show_main_window(app);
            }
            // Home calls this too once the hidden webview loads; the second
            // call is a no-op. Starting here does not depend on the webview.
            if let Err(error) = start_hosting(app.state(), app.clone()) {
                log::warn!("could not start hosting at sign-in: {error}");
            }
        }
    }
}

pub(crate) fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Bring back a hidden window, e.g. for an approval prompt. A visible window
/// is left alone so this never steals focus.
pub(crate) fn reveal_if_hidden(app: &AppHandle) {
    let hidden = app.get_webview_window("main").is_some_and(|window| matches!(window.is_visible(), Ok(false)));
    if hidden {
        show_main_window(app);
    }
}

/// Window close handler. Returns true when the close was turned into a hide.
pub(crate) fn hide_instead_of_close(window: &tauri::Window) -> bool {
    let app = window.app_handle();
    if window.label() != "main" {
        return false;
    }
    let autostart = app.autolaunch().is_enabled().unwrap_or(false);
    // Skip the keychain lookup when autostart is off.
    if !keep_running_on_close(autostart, autostart && unattended_ready(app)) {
        return false;
    }
    if let Err(error) = ensure_tray(app) {
        // Without a tray there is no way back to the window; quit instead.
        log::warn!("could not create the tray icon: {error}");
        return false;
    }
    let _ = window.hide();
    true
}

/// The tray's hosting item, kept to relabel it when the host status changes.
struct TrayHostingItem(MenuItem<tauri::Wry>);

/// The tray is only created once the window can be hidden, so a normal
/// launch looks the same as before.
pub(crate) fn ensure_tray(app: &AppHandle) -> tauri::Result<()> {
    if app.tray_by_id(TRAY_ID).is_some() {
        return Ok(());
    }
    let show = MenuItem::with_id(app, "show", "Show OpenDesk", true, None::<&str>)?;
    let hosting = MenuItem::with_id(app, "hosting", hosting_item_label(host_status()), true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit OpenDesk", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &hosting, &quit])?;
    let mut tray = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .tooltip("OpenDesk")
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => show_main_window(app),
            "hosting" => toggle_hosting(app),
            "quit" => app.exit(0),
            _ => {}
        });
    // Windows convention: left click opens the app, right click the menu.
    #[cfg(windows)]
    {
        use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
        tray = tray.show_menu_on_left_click(false).on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                show_main_window(tray.app_handle());
            }
        });
    }
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    app.manage(TrayHostingItem(hosting));
    Ok(())
}

fn toggle_hosting(app: &AppHandle) {
    let app = app.clone();
    // stop_hosting joins the host thread; keep that off the main thread,
    // which the host thread's own cleanup may need.
    tauri::async_runtime::spawn_blocking(move || {
        if host_status() == "offline" {
            if let Err(error) = start_hosting(app.state(), app.clone()) {
                log::warn!("could not start hosting from the tray: {error}");
            }
        } else {
            stop_hosting(app.state(), app.clone());
        }
    });
}

/// Keep the tray's hosting item in step with `status`. Posted to the main
/// thread rather than run inline: the caller may be the host thread while
/// the main thread waits for it to finish.
pub(crate) fn sync_tray(app: &AppHandle, status: &'static str) {
    if app.try_state::<TrayHostingItem>().is_none() {
        return;
    }
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(item) = handle.try_state::<TrayHostingItem>() {
            let _ = item.0.set_text(hosting_item_label(status));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_flag_is_detected_anywhere_in_the_args() {
        assert!(launched_in_background(["--background"]));
        assert!(launched_in_background(["--foo", "--background"]));
        assert!(!launched_in_background(Vec::<String>::new()));
        assert!(!launched_in_background(["--backgrounds", "background", "-b"]));
    }

    #[test]
    fn background_launch_hides_only_when_unattended_is_ready() {
        assert_eq!(startup_mode(true, true), Startup::HostHidden);
        assert_eq!(startup_mode(true, false), Startup::ShowWindow);
        assert_eq!(startup_mode(false, true), Startup::ShowWindow);
        assert_eq!(startup_mode(false, false), Startup::ShowWindow);
    }

    #[test]
    fn closing_quits_unless_set_up_to_stay_reachable() {
        assert!(keep_running_on_close(true, true));
        assert!(!keep_running_on_close(true, false));
        assert!(!keep_running_on_close(false, true));
        assert!(!keep_running_on_close(false, false));
    }

    #[test]
    fn hosting_item_offers_the_opposite_of_the_current_state() {
        assert_eq!(hosting_item_label("offline"), "Start hosting");
        assert_eq!(hosting_item_label("online"), "Stop hosting");
        assert_eq!(hosting_item_label("connecting"), "Stop hosting");
        assert_eq!(hosting_item_label("in_session"), "Stop hosting");
    }
}
