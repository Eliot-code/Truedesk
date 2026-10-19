//! The Truedesk viewer for phones and tablets: the browser viewer (`web/`)
//! in an app window. The session runs in the web view, so this only opens it.

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running Truedesk");
}
