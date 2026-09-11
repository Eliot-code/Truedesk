// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
use std::sync::Mutex;

use protocol::crypto::{Opener, Sealer};
use protocol::pake::{self, PakeState};

/// Handshake + session state. Held only in the Rust backend; never crosses to JS.
/// No `Debug` derive: `PakeState` has none.
#[derive(Default)]
struct Session {
    pake: Option<PakeState>,
    shared: Option<[u8; 32]>,
    opener: Option<Opener>,
    sealer: Option<Sealer>,
}

struct AppState(Mutex<Session>);

/// Begin SPAKE2 (viewer = side B). Stores state, returns the `0x01`-tagged body to send.
#[tauri::command]
fn kex_start(state: tauri::State<AppState>, password: String) -> Vec<u8> {
    let (pk, msg) = pake::start(password.as_bytes(), false); // viewer = side B
    let mut s = state.0.lock().unwrap();
    s.pake = Some(pk);
    let mut body = Vec::with_capacity(1 + msg.len());
    body.push(0x01); // KEX_MSG
    body.extend_from_slice(&msg);
    body
}

/// Complete SPAKE2, derive keys, store the recv-keyed `Opener`, return this side's
/// confirm MAC as the `0x02`-tagged body. `peer_msg` is the raw spake2 message
/// (sub-tag already stripped by the TS layer).
#[tauri::command]
fn kex_finish(state: tauri::State<AppState>, peer_msg: Vec<u8>) -> Result<Vec<u8>, String> {
    let mut s = state.0.lock().unwrap();
    let pk = s.pake.take().ok_or("kex not started")?;
    let shared = pake::finish(pk, &peer_msg).ok_or("bad spake2 message")?;
    let (send, recv) = protocol::kex::derive_keys(&shared, false); // viewer send = k_v2h, recv = k_h2v
    s.opener = Some(Opener::new(recv));
    s.sealer = Some(Sealer::new(send));
    s.shared = Some(shared);
    let confirm = pake::confirm_tag(&shared, false); // viewer role
    let mut body = Vec::with_capacity(1 + confirm.len());
    body.push(0x02); // KEX_CONFIRM
    body.extend_from_slice(&confirm);
    Ok(body)
}

/// Verify the host's confirm MAC. `peer_confirm` is the raw 32-byte MAC
/// (sub-tag already stripped by the TS layer). An error aborts the session.
#[tauri::command]
fn kex_verify(state: tauri::State<AppState>, peer_confirm: Vec<u8>) -> Result<(), String> {
    let s = state.0.lock().unwrap();
    let shared = s.shared.ok_or("no shared key")?;
    if pake::verify_peer(&shared, true, &peer_confirm) {
        // peer is host
        Ok(())
    } else {
        Err("key confirmation failed — wrong password or tampering".into())
    }
}

/// Open one sealed video body with the stored `Opener`, returning plaintext.
#[tauri::command]
fn open_frame(state: tauri::State<AppState>, sealed: Vec<u8>) -> Result<Vec<u8>, String> {
    let mut s = state.0.lock().unwrap();
    let opener = s.opener.as_mut().ok_or("no session key")?;
    opener
        .open(&sealed)
        .ok_or_else(|| "AEAD open failed".into())
}

/// Seal one plaintext input event with the viewer-to-host session key.
#[tauri::command]
fn seal_input(state: tauri::State<AppState>, bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    let mut s = state.0.lock().unwrap();
    let sealer = s.sealer.as_mut().ok_or("no session key")?;
    Ok(sealer.seal(&bytes))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState(Mutex::new(Session::default())))
        .invoke_handler(tauri::generate_handler![
            kex_start, kex_finish, kex_verify, open_frame, seal_input
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
