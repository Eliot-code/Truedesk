// The phone app on a computer, for trying it out.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    truedesk_mobile_lib::run()
}
