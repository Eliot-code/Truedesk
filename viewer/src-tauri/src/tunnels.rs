//! The session's TCP tunnels (see `host_core::tunnel`). On the host they obey
//! the viewer's tunnel permission and log every connection; on the viewer
//! they forward local ports and keep the UI's list current.

use super::*;
use host_core::tunnel::{ForwardInfo, TunnelEvent, TunnelNotify, Tunnels};
use protocol::tunnel::TunnelMsg;
use std::net::{Ipv4Addr, SocketAddr};

/// Which end of the session this app is.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum DataRole {
    Host,
    Viewer,
}

/// A port forward, for the UI.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub(crate) struct TunnelInfo {
    pub(crate) id: u64,
    pub(crate) local_port: u16,
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) connections: usize,
}

impl From<ForwardInfo> for TunnelInfo {
    fn from(forward: ForwardInfo) -> Self {
        Self { id: forward.id, local_port: forward.local_port, host: forward.host, port: forward.port, connections: forward.connections }
    }
}

/// The host refused a connection made to the forward on `local_port`.
#[derive(Serialize, Clone)]
pub(crate) struct TunnelRefusedEvent {
    pub(crate) local_port: u16,
    pub(crate) code: String,
}

/// Start this session's tunnels: what they queue is sealed and sent, in
/// order, on the Data channel. A host also tells the viewer it takes tunnels.
pub(crate) fn start_tunnels(app: &AppHandle, role: DataRole, sender: ReliableSender, sealer: Arc<Mutex<Sealer>>, allowed: Arc<AtomicBool>) -> Tunnels {
    let (out, mut outbox) = mpsc::channel::<TunnelMsg>(host_core::tunnel::OUTBOX_CAPACITY);
    let events_app = app.clone();
    let notify: TunnelNotify = Arc::new(move |event| on_tunnel_event(&events_app, event));
    // Tunnel tasks run on the app's runtime, not on a host session's thread.
    let runtime = tauri::async_runtime::handle();
    let _runtime = runtime.inner().enter();
    let tunnels = match role {
        DataRole::Host => Tunnels::host(out, allowed, notify),
        DataRole::Viewer => Tunnels::viewer(out, notify),
    };
    let announce = (role == DataRole::Host).then(|| tunnels.clone());
    tauri::async_runtime::spawn(async move {
        if let Some(tunnels) = announce { tunnels.announce().await; }
        while let Some(message) = outbox.recv().await {
            let sealed = lock_unpoisoned(&sealer).seal(&[Tag::Data as u8], &DataMsg::Tunnel(message).to_bytes());
            if sender.send_frame(Tag::Data, &sealed).await.is_err() { break; }
        }
    });
    tunnels
}

fn on_tunnel_event(app: &AppHandle, event: TunnelEvent) {
    match event {
        TunnelEvent::Connected { target } => emit_activity(app, "tunnel", format!("tunnel opened to {target}")),
        TunnelEvent::Refused { target, code } => emit_activity(app, "tunnel", format!("tunnel to {target} refused ({code})")),
        TunnelEvent::Available => emit(app, "tunnel_support", ()),
        TunnelEvent::ForwardRefused { local_port, code } => emit(app, "tunnel_refused", TunnelRefusedEvent { local_port, code }),
        // The UI reads the list with `list_tunnels`; this event carries no
        // list so it never needs the app state lock.
        TunnelEvent::ForwardsChanged => emit(app, "tunnels_changed", ()),
    }
}

fn current_tunnels(state: &tauri::State<'_, AppState>) -> Result<Tunnels, String> {
    state.lock().tunnels.clone().ok_or_else(|| "Connect to a computer first.".to_owned())
}

/// What to tell the user when a local port can't be opened.
fn listen_error(error: &io::Error, local_port: u16) -> String {
    match error.kind() {
        io::ErrorKind::AddrInUse => format!("Port {local_port} is already in use on this computer. Choose another, or leave it empty to pick a free one."),
        io::ErrorKind::PermissionDenied => format!("This computer doesn't let Truedesk use port {local_port}. Choose a port above 1024."),
        _ => error.to_string(),
    }
}

/// Viewer: listen on `local_port` (0 picks a free one) on this computer and
/// carry every connection to `host:port` on the other computer's side.
#[tauri::command]
pub(crate) async fn open_tunnel(state: tauri::State<'_, AppState>, local_port: u16, host: String, port: u16) -> Result<TunnelInfo, String> {
    let tunnels = current_tunnels(&state)?;
    let local = SocketAddr::from((Ipv4Addr::LOCALHOST, local_port));
    tunnels.forward(local, &host, port).await.map(TunnelInfo::from).map_err(|error| listen_error(&error, local_port))
}

/// Viewer: stop tunnel `id` and close its connections.
#[tauri::command]
pub(crate) fn close_tunnel(state: tauri::State<'_, AppState>, id: u64) -> Result<(), String> {
    current_tunnels(&state)?.stop_forward(id);
    Ok(())
}

/// Viewer: whether the host of the current session takes tunnels. The UI
/// asks when it opens, in case it missed the `tunnel_support` event.
#[tauri::command]
pub(crate) fn tunnel_support(state: tauri::State<'_, AppState>) -> bool {
    state.lock().tunnels.as_ref().is_some_and(|tunnels| tunnels.available())
}

/// Viewer: the tunnels of the current session.
#[tauri::command]
pub(crate) fn list_tunnels(state: tauri::State<'_, AppState>) -> Vec<TunnelInfo> {
    let tunnels = state.lock().tunnels.clone();
    tunnels.map(|tunnels| tunnels.forwards().into_iter().map(TunnelInfo::from).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listen_errors_say_what_to_do() {
        assert!(listen_error(&io::Error::from(io::ErrorKind::AddrInUse), 3389).contains("Port 3389 is already in use"));
        assert!(listen_error(&io::Error::from(io::ErrorKind::PermissionDenied), 80).contains("above 1024"));
        assert_eq!(listen_error(&io::Error::new(io::ErrorKind::InvalidInput, "bad target"), 1), "bad target");
    }

    #[test]
    fn forwards_convert_for_the_ui() {
        let info = TunnelInfo::from(ForwardInfo { id: 3, local_port: 13389, host: "10.0.0.5".into(), port: 3389, connections: 2 });
        assert_eq!(info, TunnelInfo { id: 3, local_port: 13389, host: "10.0.0.5".into(), port: 3389, connections: 2 });
    }
}
