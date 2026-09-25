mod registry;
mod session;

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use registry::{Outbound, Registry};
use session::Bridge;
use std::borrow::Cow;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::sync::watch;

const DEFAULT_MAX_CONNECTIONS: usize = 10_000;
const DEFAULT_MAX_CONNECTIONS_PER_IP: usize = 50;
const DEFAULT_SIGNAL_CAPACITY: u32 = 6;
const DEFAULT_SIGNAL_REFILL_SECS: u64 = 10;
const DEFAULT_BYTES_PER_SEC: u64 = 64 * 1024 * 1024;
const DEFAULT_BYTE_BURST: u64 = 8 * 1024 * 1024;
const DEFAULT_HANDSHAKE_TIMEOUT_SECS: u64 = 15;
const DEFAULT_PAIR_TIMEOUT_SECS: u64 = 60;
const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 300;

#[derive(Clone)]
pub struct RelayState {
    registry: Registry,
    limits: Arc<ConnectionLimits>,
    config: RelayConfig,
    shutdown: watch::Receiver<bool>,
    _shutdown_keepalive: Option<watch::Sender<bool>>,
}

#[derive(Clone, Debug)]
struct RelayConfig {
    signal_capacity: u32,
    signal_refill: Duration,
    bytes_per_sec: u64,
    byte_burst: u64,
    handshake_timeout: Duration,
    pair_timeout: Duration,
    idle_timeout: Duration,
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self {
            signal_capacity: DEFAULT_SIGNAL_CAPACITY,
            signal_refill: Duration::from_secs(DEFAULT_SIGNAL_REFILL_SECS),
            bytes_per_sec: DEFAULT_BYTES_PER_SEC,
            byte_burst: DEFAULT_BYTE_BURST,
            handshake_timeout: Duration::from_secs(DEFAULT_HANDSHAKE_TIMEOUT_SECS),
            pair_timeout: Duration::from_secs(DEFAULT_PAIR_TIMEOUT_SECS),
            idle_timeout: Duration::from_secs(DEFAULT_IDLE_TIMEOUT_SECS),
        }
    }
}

impl RelayConfig {
    fn from_env() -> Self {
        let default = Self::default();
        Self {
            signal_capacity: env_u64("RELAY_SIGNAL_BURST", default.signal_capacity as u64) as u32,
            signal_refill: Duration::from_secs(env_u64("RELAY_SIGNAL_REFILL_SECS", DEFAULT_SIGNAL_REFILL_SECS)),
            bytes_per_sec: env_u64("RELAY_MAX_BYTES_PER_SEC", DEFAULT_BYTES_PER_SEC),
            byte_burst: env_u64("RELAY_BYTE_BURST", DEFAULT_BYTE_BURST),
            handshake_timeout: Duration::from_secs(env_u64("RELAY_HANDSHAKE_TIMEOUT_SECS", DEFAULT_HANDSHAKE_TIMEOUT_SECS)),
            pair_timeout: Duration::from_secs(env_u64("RELAY_PAIR_TIMEOUT_SECS", DEFAULT_PAIR_TIMEOUT_SECS)),
            idle_timeout: Duration::from_secs(env_u64("RELAY_IDLE_TIMEOUT_SECS", DEFAULT_IDLE_TIMEOUT_SECS)),
        }
    }
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).filter(|v| *v > 0).unwrap_or(default)
}

fn env_usize(name: &str, default: usize) -> usize { env_u64(name, default as u64) as usize }

struct ConnectionLimits {
    max: usize,
    per_ip: usize,
    active: AtomicUsize,
    ips: Mutex<HashMap<IpAddr, usize>>,
}

impl ConnectionLimits {
    fn from_env() -> Self {
        Self {
            max: env_usize("RELAY_MAX_CONNECTIONS", DEFAULT_MAX_CONNECTIONS),
            per_ip: env_usize("RELAY_MAX_CONNECTIONS_PER_IP", DEFAULT_MAX_CONNECTIONS_PER_IP),
            active: AtomicUsize::new(0),
            ips: Mutex::new(HashMap::new()),
        }
    }

    fn try_acquire(self: &Arc<Self>, ip: IpAddr) -> Option<ConnectionGuard> {
        let mut ips = self.ips.lock().expect("connection limits lock poisoned");
        let current = ips.get(&ip).copied().unwrap_or(0);
        if self.active.load(Ordering::Relaxed) >= self.max || current >= self.per_ip { return None; }
        self.active.fetch_add(1, Ordering::Relaxed);
        *ips.entry(ip).or_default() += 1;
        Some(ConnectionGuard { limits: Arc::clone(self), ip })
    }

    fn active(&self) -> usize { self.active.load(Ordering::Relaxed) }
}

struct ConnectionGuard { limits: Arc<ConnectionLimits>, ip: IpAddr }

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.limits.active.fetch_sub(1, Ordering::Relaxed);
        let mut ips = self.limits.ips.lock().expect("connection limits lock poisoned");
        if let Some(count) = ips.get_mut(&self.ip) {
            *count -= 1;
            if *count == 0 { ips.remove(&self.ip); }
        }
    }
}

#[derive(Clone, Debug)]
struct TokenBucket { tokens: f64, capacity: f64, refill_per_sec: f64, last: Instant }

impl TokenBucket {
    fn new(capacity: u64, refill_per_sec: u64) -> Self {
        Self { tokens: capacity as f64, capacity: capacity as f64, refill_per_sec: refill_per_sec as f64, last: Instant::now() }
    }

    fn allow(&mut self, amount: usize) -> bool {
        let now = Instant::now();
        self.tokens = (self.tokens + now.duration_since(self.last).as_secs_f64() * self.refill_per_sec).min(self.capacity);
        self.last = now;
        if self.tokens >= amount as f64 { self.tokens -= amount as f64; true } else { false }
    }
}

pub fn app() -> Router {
    let (tx, rx) = watch::channel(false);
    app_with_state(RelayState { registry: Registry::default(), limits: Arc::new(ConnectionLimits::from_env()), config: RelayConfig::from_env(), shutdown: rx, _shutdown_keepalive: Some(tx) })
}

pub fn app_with_shutdown(shutdown: watch::Receiver<bool>) -> Router {
    app_with_state(RelayState { registry: Registry::default(), limits: Arc::new(ConnectionLimits::from_env()), config: RelayConfig::from_env(), shutdown, _shutdown_keepalive: None })
}

fn app_with_state(state: RelayState) -> Router {
    Router::new().route("/ws", get(ws_handler)).route("/health", get(health_handler)).with_state(state)
}

async fn health_handler(State(state): State<RelayState>) -> Response {
    let body = format!(r#"{{"status":"ok","active_connections":{}}}"#, state.limits.active());
    ([(axum::http::header::CONTENT_TYPE, "application/json")], body).into_response()
}

async fn ws_handler(ws: WebSocketUpgrade, ConnectInfo(addr): ConnectInfo<SocketAddr>, State(state): State<RelayState>) -> Response {
    let guard = state.limits.try_acquire(addr.ip());
    if guard.is_none() { log_event("connection_rejected", addr, "capacity"); }
    ws.on_upgrade(move |socket| handle(socket, state, addr, guard))
}

async fn handle(socket: WebSocket, state: RelayState, addr: SocketAddr, guard: Option<ConnectionGuard>) {
    let Some(_guard) = guard else {
        let mut socket = socket;
        let _ = socket.send(Message::Close(Some(CloseFrame { code: 1013, reason: Cow::Borrowed("relay capacity") }))).await;
        return;
    };
    log_event("connection_open", addr, "accepted");
    let (mut sink, mut stream) = socket.split();
    let (my_tx, mut my_rx) = mpsc::channel::<Vec<u8>>(256);
    let writer = tokio::spawn(async move {
        while let Some(bytes) = my_rx.recv().await {
            if sink.send(Message::Binary(bytes)).await.is_err() { break; }
        }
    });

    let mut shutdown = state.shutdown.clone();
    let first = tokio::select! {
        _ = shutdown.changed() => { writer.abort(); log_event("connection_close", addr, "shutdown"); return; }
        result = tokio::time::timeout(state.config.handshake_timeout, stream.next()) => result.ok().flatten().and_then(Result::ok),
    };
    let Some(Message::Binary(first)) = first else { writer.abort(); log_event("connection_close", addr, "handshake"); return; };
    let refill = state.config.signal_capacity as u64 / state.config.signal_refill.as_secs().max(1);
    let mut signaling = TokenBucket::new(state.config.signal_capacity as u64, refill.max(1));
    if !signaling.allow(1) { writer.abort(); log_event("connection_rejected", addr, "signaling_rate"); return; }
    let mut bytes = TokenBucket::new(state.config.byte_burst, state.config.bytes_per_sec);
    if !bytes.allow(first.len()) { writer.abort(); log_event("connection_rejected", addr, "byte_rate"); return; }

    let bridge = session::classify(&first, my_tx.clone(), &state.registry);
    let (peer_tx, registered_peer) = match bridge {
        Some(Bridge::ViewerPaired { host_tx, requester_id, target_id }) => {
            let _ = host_tx.send(session::paired_frame("host", &requester_id)).await;
            let _ = my_tx.send(session::paired_frame("viewer", &target_id)).await;
            log_event("pairing_success", addr, "viewer");
            (host_tx, None)
        }
        Some(Bridge::HostRegistered { peer_id, notify_rx }) => {
            let peer = tokio::select! {
                _ = shutdown.changed() => { state.registry.unregister(&peer_id, &my_tx); writer.abort(); log_event("connection_close", addr, "shutdown"); return; }
                result = tokio::time::timeout(state.config.pair_timeout, notify_rx) => result.ok().and_then(Result::ok),
            };
            let Some((peer, requester_id)) = peer else { state.registry.unregister(&peer_id, &my_tx); writer.abort(); log_event("pairing_failure", addr, "pair_timeout"); return; };
            let _ = my_tx.send(session::paired_frame("host", &requester_id)).await;
            log_event("pairing_success", addr, "host");
            (peer, Some(peer_id))
        }
        None => { writer.abort(); log_event("pairing_failure", addr, "invalid_control"); return; }
    };
    forward_rest(&mut stream, peer_tx, &mut bytes, &mut signaling, &state, &mut shutdown, addr).await;
    if let Some(peer_id) = registered_peer { state.registry.unregister(&peer_id, &my_tx); }
    writer.abort();
    log_event("connection_close", addr, "eof_or_timeout");
}

async fn forward_rest(stream: &mut futures_util::stream::SplitStream<WebSocket>, peer_tx: Outbound, bytes: &mut TokenBucket, signaling: &mut TokenBucket, state: &RelayState, shutdown: &mut watch::Receiver<bool>, addr: SocketAddr) {
    loop {
        let next = tokio::select! {
            _ = shutdown.changed() => break,
            result = tokio::time::timeout(state.config.idle_timeout, stream.next()) => match result { Ok(value) => value, Err(_) => { log_event("connection_close", addr, "idle_timeout"); break; } },
        };
        let Some(Ok(msg)) = next else { break; };
        if let Message::Binary(frame) = msg {
            if !bytes.allow(frame.len()) { log_event("connection_rejected", addr, "byte_rate"); break; }
            if frame.first() == Some(&(protocol::Tag::Control as u8)) && !signaling.allow(1) { log_event("connection_rejected", addr, "signaling_rate"); break; }
            let queued = tokio::select! {
                _ = shutdown.changed() => false,
                result = tokio::time::timeout(state.config.idle_timeout, peer_tx.send(frame)) => result.is_ok(),
            };
            if !queued { break; }
        }
    }
}

fn log_event(event: &str, addr: SocketAddr, reason: &str) { eprintln!("[relay] event={event} remote={addr} reason={reason}"); }

pub async fn spawn_test_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move { axum::serve(listener, app().into_make_service_with_connect_info::<SocketAddr>()).await.unwrap() });
    addr
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn token_bucket_allows_burst_then_refills() {
        let mut bucket = TokenBucket::new(2, 20);
        assert!(bucket.allow(1)); assert!(bucket.allow(1)); assert!(!bucket.allow(1));
        std::thread::sleep(Duration::from_millis(75));
        assert!(bucket.allow(1));
    }

    #[test]
    fn connection_limits_enforce_global_and_per_ip_caps() {
        let limits = Arc::new(ConnectionLimits { max: 2, per_ip: 1, active: AtomicUsize::new(0), ips: Mutex::new(HashMap::new()) });
        let first = limits.try_acquire(IpAddr::V4(Ipv4Addr::LOCALHOST)).unwrap();
        assert!(limits.try_acquire(IpAddr::V4(Ipv4Addr::LOCALHOST)).is_none());
        let second = limits.try_acquire(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2))).unwrap();
        assert_eq!(limits.active(), 2);
        assert!(limits.try_acquire(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 3))).is_none());
        drop(first); drop(second);
        assert_eq!(limits.active(), 0);
    }

    #[test]
    fn configured_deadline_is_positive() { assert!(RelayConfig::default().pair_timeout > Duration::ZERO); }
}
