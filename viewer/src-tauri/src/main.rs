// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(windows)]
    if viewer_lib::run_windows_service() {
        return;
    }
    viewer_lib::run()
}
