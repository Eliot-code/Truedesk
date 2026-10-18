//! TCP tunnels (port forwarding) over a session's Data channel.
//!
//! The viewer listens on a local port ([`Tunnels::forward`]); each connection
//! to it asks the host to connect to a target on the host's network. The host
//! does that only while its tunnel permission is on. Every connection then
//! carries bytes both ways under the flow control described in
//! [`protocol::tunnel`].
//!
//! [`Tunnels::route`] takes each inbound tunnel message and never blocks, so
//! the session's Data dispatch can call it directly. Create a `Tunnels` inside
//! a Tokio runtime; its methods may then be called from any thread. Outbound messages go to
//! the `out` queue given at construction; the app seals and sends them, in
//! order, on the Data channel.

use protocol::tunnel::{TunnelMsg, MAX_TUNNEL_CHUNK, MAX_TUNNEL_HOST, TUNNEL_INVALID, TUNNEL_LIMIT, TUNNEL_NOT_ALLOWED, TUNNEL_UNREACHABLE, TUNNEL_WINDOW};
use std::{
    collections::HashMap,
    io,
    net::SocketAddr,
    sync::{atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering}, Arc, Mutex, MutexGuard},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    runtime::Handle,
    sync::{mpsc, oneshot, watch},
    task::{AbortHandle, JoinSet},
};

/// Tunnel connections one session may have open at once.
pub const MAX_TUNNEL_CONNECTIONS: usize = 32;
/// Port forwards a viewer may have listening at once.
pub const MAX_FORWARDS: usize = 16;
/// A good size for the `out` queue.
pub const OUTBOX_CAPACITY: usize = 64;
/// `ForwardRefused` code when the host never answered an `Open`.
pub const TUNNEL_NO_ANSWER: &str = "no_answer";
/// How long the host tries to reach a target.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a viewer connection waits for the host's answer.
const OPEN_TIMEOUT: Duration = Duration::from_secs(20);
/// The writer acknowledges at least every this many bytes.
const ACK_EVERY: u64 = TUNNEL_WINDOW / 4;

/// Something the app should tell its user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TunnelEvent {
    /// Host: the viewer opened a connection to `target`.
    Connected { target: String },
    /// Host: a connection to `target` the viewer asked for was refused;
    /// `code` is a `TUNNEL_*` code.
    Refused { target: String, code: String },
    /// Viewer: the host takes tunnel connections.
    Available,
    /// Viewer: the host refused a connection made to the forward on
    /// `local_port`; `code` is a `TUNNEL_*` code or [`TUNNEL_NO_ANSWER`].
    ForwardRefused { local_port: u16, code: String },
    /// Viewer: a forward was added or removed, or its connections changed;
    /// see [`Tunnels::forwards`].
    ForwardsChanged,
}

pub type TunnelNotify = Arc<dyn Fn(TunnelEvent) + Send + Sync>;

/// A port forward listening on the viewer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardInfo {
    pub id: u64,
    /// The local port; connect to it on 127.0.0.1.
    pub local_port: u16,
    /// The target, as the host resolves it.
    pub host: String,
    pub port: u16,
    /// Connections open through it right now.
    pub connections: usize,
}

/// One side of a session's tunnels. Clones share the same state.
#[derive(Clone)]
pub struct Tunnels(Arc<Inner>);

struct Inner {
    out: mpsc::Sender<TunnelMsg>,
    /// Host: whether the viewer may open connections. `None` on a viewer,
    /// which ignores `Open`.
    allowed: Option<Arc<AtomicBool>>,
    conns: Mutex<HashMap<u64, Conn>>,
    forwards: Mutex<HashMap<u64, Forward>>,
    /// Every task this side runs; aborted together by [`Tunnels::shutdown`].
    tasks: Mutex<JoinSet<()>>,
    /// The runtime the tasks run on, so callers outside it can start them.
    runtime: Handle,
    next_id: AtomicU64,
    /// Viewer: the host said it takes tunnels.
    available: AtomicBool,
    notify: TunnelNotify,
}

struct Conn {
    inbound: mpsc::UnboundedSender<Inbound>,
    /// Bytes in `inbound` not yet written to the socket. The peer may have at
    /// most a window of them in flight.
    queued: Arc<AtomicU64>,
    /// How much of our data the peer has written to its socket.
    acked: watch::Sender<u64>,
    /// Viewer: the answer to this connection's `Open`.
    opened: Option<oneshot::Sender<Result<(), String>>>,
    /// Viewer: the forward the connection came in on.
    forward: Option<u64>,
    task: Option<AbortHandle>,
}

/// The receiving ends of a registered connection.
struct ConnEnds {
    inbound: mpsc::UnboundedReceiver<Inbound>,
    queued: Arc<AtomicU64>,
    acked: watch::Receiver<u64>,
}

/// What reaches a connection from the peer (or from us), in order.
enum Inbound {
    Data(Vec<u8>),
    Eof,
    /// The peer is done with the connection.
    Close,
    /// We end the connection and tell the peer.
    Abort,
}

struct Forward {
    local_port: u16,
    host: String,
    port: u16,
    connections: Arc<AtomicUsize>,
    task: AbortHandle,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// `host` without the brackets of an IPv6 literal, if it names a target the
/// host may try: letters, digits and the punctuation of names and addresses.
fn target_host(host: &str) -> Option<&str> {
    let host = host.trim();
    let host = host.strip_prefix('[').and_then(|inner| inner.strip_suffix(']')).unwrap_or(host);
    let valid = !host.is_empty()
        && host.len() <= MAX_TUNNEL_HOST
        && host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '%'));
    valid.then_some(host)
}

/// `host:port` for messages, with an IPv6 literal in brackets.
pub fn display_target(host: &str, port: u16) -> String {
    if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") }
}

impl Tunnels {
    /// The host side: opens the connections the viewer asks for while
    /// `allowed` is on.
    pub fn host(out: mpsc::Sender<TunnelMsg>, allowed: Arc<AtomicBool>, notify: TunnelNotify) -> Self {
        Self::new(out, Some(allowed), notify)
    }

    /// The viewer side: forwards local ports through the host.
    pub fn viewer(out: mpsc::Sender<TunnelMsg>, notify: TunnelNotify) -> Self {
        Self::new(out, None, notify)
    }

    fn new(out: mpsc::Sender<TunnelMsg>, allowed: Option<Arc<AtomicBool>>, notify: TunnelNotify) -> Self {
        Self(Arc::new(Inner {
            out,
            allowed,
            conns: Mutex::default(),
            forwards: Mutex::default(),
            tasks: Mutex::new(JoinSet::new()),
            runtime: Handle::current(),
            next_id: AtomicU64::new(1),
            available: AtomicBool::new(false),
            notify,
        }))
    }

    /// Host: tell the viewer it can open tunnels.
    pub async fn announce(&self) -> bool {
        self.0.allowed.is_some() && self.0.out.send(TunnelMsg::Available).await.is_ok()
    }

    /// Viewer: whether the host said it takes tunnels.
    pub fn available(&self) -> bool {
        self.0.available.load(Ordering::Relaxed)
    }

    /// Handle one inbound tunnel message. Never blocks.
    pub fn route(&self, message: TunnelMsg) {
        let inner = &self.0;
        match message {
            TunnelMsg::Available => {
                if inner.allowed.is_none() {
                    inner.available.store(true, Ordering::Relaxed);
                    (inner.notify)(TunnelEvent::Available);
                }
            }
            TunnelMsg::Open { id, host, port } => {
                if let Some(allowed) = &inner.allowed {
                    let allowed = Arc::clone(allowed);
                    inner.spawn(Arc::clone(inner).accept(allowed, id, host, port));
                }
            }
            TunnelMsg::Opened { id } => inner.answer(id, Ok(())),
            TunnelMsg::Refused { id, code } => inner.answer(id, Err(code)),
            TunnelMsg::Data { id, data } => inner.deliver(id, Inbound::Data(data)),
            TunnelMsg::Ack { id, written } => {
                if let Some(conn) = locked(&inner.conns).get(&id) {
                    conn.acked.send_if_modified(|acked| {
                        let newer = written > *acked;
                        if newer { *acked = written; }
                        newer
                    });
                }
            }
            TunnelMsg::Eof { id } => inner.deliver(id, Inbound::Eof),
            TunnelMsg::Close { id } => inner.deliver(id, Inbound::Close),
        }
    }

    /// Viewer: listen on `local` and carry every connection made to it to
    /// `host:port`, as the host resolves it.
    pub async fn forward(&self, local: SocketAddr, host: &str, port: u16) -> io::Result<ForwardInfo> {
        let inner = &self.0;
        let host = target_host(host).filter(|_| port != 0).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "enter a valid host name or address and a port from 1 to 65535"))?.to_owned();
        if inner.allowed.is_some() {
            return Err(io::Error::new(io::ErrorKind::Unsupported, "only the viewer forwards ports"));
        }
        if locked(&inner.forwards).len() >= MAX_FORWARDS {
            return Err(io::Error::other(format!("at most {MAX_FORWARDS} tunnels can be open at once")));
        }
        let listener = TcpListener::bind(local).await?;
        let local_port = listener.local_addr()?.port();
        let id = inner.next_id.fetch_add(1, Ordering::Relaxed);
        let connections = Arc::new(AtomicUsize::new(0));
        let task = inner.spawn(Arc::clone(inner).listen(id, listener, local_port, host.clone(), port, Arc::clone(&connections)));
        locked(&inner.forwards).insert(id, Forward { local_port, host: host.clone(), port, connections, task });
        (inner.notify)(TunnelEvent::ForwardsChanged);
        Ok(ForwardInfo { id, local_port, host, port, connections: 0 })
    }

    /// Viewer: the forwards listening now, oldest first.
    pub fn forwards(&self) -> Vec<ForwardInfo> {
        let mut forwards: Vec<ForwardInfo> = locked(&self.0.forwards)
            .iter()
            .map(|(id, forward)| ForwardInfo {
                id: *id,
                local_port: forward.local_port,
                host: forward.host.clone(),
                port: forward.port,
                connections: forward.connections.load(Ordering::Relaxed),
            })
            .collect();
        forwards.sort_by_key(|forward| forward.id);
        forwards
    }

    /// Viewer: stop listening on forward `id` and close its connections.
    /// Returns whether it existed.
    pub fn stop_forward(&self, id: u64) -> bool {
        let Some(forward) = locked(&self.0.forwards).remove(&id) else { return false };
        forward.task.abort();
        self.0.abort_where(|conn| conn.forward == Some(id));
        (self.0.notify)(TunnelEvent::ForwardsChanged);
        true
    }

    /// Close every connection and tell the peer, e.g. when the host turns
    /// tunnels off. Forwards keep listening.
    pub fn close_all(&self) {
        self.0.abort_where(|_| true);
    }

    /// End everything without telling the peer: the session is over.
    pub fn shutdown(&self) {
        locked(&self.0.tasks).abort_all();
        locked(&self.0.conns).clear();
        let had_forwards = !std::mem::take(&mut *locked(&self.0.forwards)).is_empty();
        if had_forwards { (self.0.notify)(TunnelEvent::ForwardsChanged); }
    }
}

impl Inner {
    fn spawn(&self, task: impl std::future::Future<Output = ()> + Send + 'static) -> AbortHandle {
        let mut tasks = locked(&self.tasks);
        // Reap finished tasks so a long session doesn't keep their results.
        while tasks.try_join_next().is_some() {}
        tasks.spawn_on(task, &self.runtime)
    }

    /// Queue `message` for the peer without waiting.
    fn tell(&self, message: TunnelMsg) {
        if let Err(mpsc::error::TrySendError::Full(message)) = self.out.try_send(message) {
            let out = self.out.clone();
            self.spawn(async move { let _ = out.send(message).await; });
        }
    }

    /// Track connection `id`. `None` when the session has no room left or
    /// `id` is in use.
    fn register(&self, id: u64, forward: Option<u64>, opened: Option<oneshot::Sender<Result<(), String>>>) -> Option<ConnEnds> {
        let mut conns = locked(&self.conns);
        if conns.len() >= MAX_TUNNEL_CONNECTIONS || conns.contains_key(&id) {
            return None;
        }
        let (inbound_tx, inbound) = mpsc::unbounded_channel();
        let (acked_tx, acked) = watch::channel(0);
        let queued = Arc::new(AtomicU64::new(0));
        conns.insert(id, Conn { inbound: inbound_tx, queued: Arc::clone(&queued), acked: acked_tx, opened, forward, task: None });
        Some(ConnEnds { inbound, queued, acked })
    }

    fn set_task(&self, id: u64, task: AbortHandle) {
        if let Some(conn) = locked(&self.conns).get_mut(&id) { conn.task = Some(task); }
    }

    fn forget(&self, id: u64) {
        locked(&self.conns).remove(&id);
    }

    /// End the connections matching `which` and tell the peer.
    fn abort_where(&self, which: impl Fn(&Conn) -> bool) {
        let ended: Vec<(u64, Conn)> = {
            let mut conns = locked(&self.conns);
            let ids: Vec<u64> = conns.iter().filter(|(_, conn)| which(conn)).map(|(id, _)| *id).collect();
            ids.into_iter().filter_map(|id| conns.remove(&id).map(|conn| (id, conn))).collect()
        };
        for (id, conn) in ended {
            if let Some(task) = conn.task { task.abort(); }
            self.tell(TunnelMsg::Close { id });
        }
    }

    /// Pass an event to connection `id`, enforcing the peer's window.
    fn deliver(&self, id: u64, event: Inbound) {
        let conns = locked(&self.conns);
        let Some(conn) = conns.get(&id) else { return };
        if let Inbound::Data(data) = &event {
            let queued = conn.queued.fetch_add(data.len() as u64, Ordering::AcqRel) + data.len() as u64;
            if queued > TUNNEL_WINDOW {
                log::warn!("closing tunnel {id}: the peer sent past its window");
                let _ = conn.inbound.send(Inbound::Abort);
                return;
            }
        }
        let _ = conn.inbound.send(event);
    }

    /// Viewer: the host answered the `Open` for connection `id`.
    fn answer(&self, id: u64, result: Result<(), String>) {
        let mut conns = locked(&self.conns);
        match conns.get_mut(&id) {
            Some(conn) => {
                if let Some(opened) = conn.opened.take() { let _ = opened.send(result); }
            }
            // We gave up waiting; don't leave the host holding a connection.
            None if result.is_ok() => {
                drop(conns);
                self.tell(TunnelMsg::Close { id });
            }
            None => {}
        }
    }

    /// Host: connect to the target the viewer asked for and carry the
    /// connection.
    async fn accept(self: Arc<Self>, allowed: Arc<AtomicBool>, id: u64, host: String, port: u16) {
        let target = display_target(&host, port);
        let refusal = if !allowed.load(Ordering::Relaxed) {
            Some(TUNNEL_NOT_ALLOWED)
        } else if target_host(&host).is_none() || port == 0 {
            Some(TUNNEL_INVALID)
        } else {
            None
        };
        if let Some(code) = refusal {
            return self.refuse(id, target, code).await;
        }
        if locked(&self.conns).contains_key(&id) {
            return; // A repeated Open; the first one is being handled.
        }
        let Some(ends) = self.register(id, None, None) else {
            return self.refuse(id, target, TUNNEL_LIMIT).await;
        };
        let address = target_host(&host).unwrap_or_default().to_owned();
        let stream = match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((address.as_str(), port))).await {
            Ok(Ok(stream)) => stream,
            Ok(Err(error)) => {
                log::info!("tunnel to {target} failed: {error}");
                self.forget(id);
                return self.refuse(id, target, TUNNEL_UNREACHABLE).await;
            }
            Err(_) => {
                self.forget(id);
                return self.refuse(id, target, TUNNEL_UNREACHABLE).await;
            }
        };
        if !locked(&self.conns).contains_key(&id) {
            return; // Closed while we were connecting.
        }
        if self.out.send(TunnelMsg::Opened { id }).await.is_err() {
            return self.forget(id);
        }
        (self.notify)(TunnelEvent::Connected { target });
        self.carry(id, stream, ends).await;
    }

    async fn refuse(&self, id: u64, target: String, code: &str) {
        let _ = self.out.send(TunnelMsg::Refused { id, code: code.to_owned() }).await;
        (self.notify)(TunnelEvent::Refused { target, code: code.to_owned() });
    }

    /// Viewer: accept connections on a forward until it is stopped.
    async fn listen(self: Arc<Self>, forward: u64, listener: TcpListener, local_port: u16, host: String, port: u16, connections: Arc<AtomicUsize>) {
        loop {
            let socket = match listener.accept().await {
                Ok((socket, _)) => socket,
                Err(error) => {
                    // Out of descriptors and the like; don't spin.
                    log::warn!("tunnel listener on port {local_port}: {error}");
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    continue;
                }
            };
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            let (opened_tx, opened) = oneshot::channel();
            let Some(ends) = self.register(id, Some(forward), Some(opened_tx)) else {
                (self.notify)(TunnelEvent::ForwardRefused { local_port, code: TUNNEL_LIMIT.into() });
                continue; // Dropping the socket closes it.
            };
            let task = self.spawn(Arc::clone(&self).open(id, socket, opened, ends, local_port, host.clone(), port, Arc::clone(&connections)));
            self.set_task(id, task);
        }
    }

    /// Viewer: ask the host to open connection `id`, then carry it.
    #[allow(clippy::too_many_arguments)]
    async fn open(self: Arc<Self>, id: u64, socket: TcpStream, opened: oneshot::Receiver<Result<(), String>>, ends: ConnEnds, local_port: u16, host: String, port: u16, connections: Arc<AtomicUsize>) {
        if self.out.send(TunnelMsg::Open { id, host, port }).await.is_err() {
            return self.forget(id);
        }
        let code = match tokio::time::timeout(OPEN_TIMEOUT, opened).await {
            Ok(Ok(Ok(()))) => None,
            Ok(Ok(Err(code))) => Some(code),
            // Gone (the session or forward ended) or no answer.
            Ok(Err(_)) => return,
            Err(_) => {
                self.forget(id);
                self.tell(TunnelMsg::Close { id });
                Some(TUNNEL_NO_ANSWER.to_owned())
            }
        };
        if let Some(code) = code {
            self.forget(id);
            (self.notify)(TunnelEvent::ForwardRefused { local_port, code });
            return;
        }
        connections.fetch_add(1, Ordering::Relaxed);
        (self.notify)(TunnelEvent::ForwardsChanged);
        self.carry(id, socket, ends).await;
        connections.fetch_sub(1, Ordering::Relaxed);
        (self.notify)(TunnelEvent::ForwardsChanged);
    }

    /// Carry an open connection both ways until both sides have finished
    /// or either fails, then forget it.
    async fn carry(&self, id: u64, stream: TcpStream, ends: ConnEnds) {
        let ConnEnds { mut inbound, queued, mut acked } = ends;
        let _ = stream.set_nodelay(true);
        let (mut reader, mut writer) = stream.into_split();
        let mut buffer = vec![0u8; MAX_TUNNEL_CHUNK];
        // Our bytes sent to the peer, the peer's bytes written to our
        // socket, and how many of those we have acknowledged.
        let (mut sent, mut written, mut reported) = (0u64, 0u64, 0u64);
        let (mut reading, mut writing) = (true, true);
        let tell_peer = loop {
            let room = TUNNEL_WINDOW.saturating_sub(sent.saturating_sub(*acked.borrow()));
            let limit = usize::try_from(room).map_or(MAX_TUNNEL_CHUNK, |room| room.min(MAX_TUNNEL_CHUNK));
            tokio::select! {
                read = reader.read(&mut buffer[..limit]), if reading && limit > 0 => match read {
                    Ok(0) => {
                        reading = false;
                        if self.out.send(TunnelMsg::Eof { id }).await.is_err() { break false; }
                    }
                    Ok(count) => {
                        sent += count as u64;
                        if self.out.send(TunnelMsg::Data { id, data: buffer[..count].to_vec() }).await.is_err() { break false; }
                    }
                    Err(_) => break true,
                },
                changed = acked.changed(), if reading && limit == 0 => {
                    if changed.is_err() { break false; }
                }
                event = inbound.recv() => match event {
                    Some(Inbound::Data(data)) => {
                        if writer.write_all(&data).await.is_err() { break true; }
                        queued.fetch_sub(data.len() as u64, Ordering::AcqRel);
                        written += data.len() as u64;
                        if written - reported >= ACK_EVERY || inbound.is_empty() {
                            reported = written;
                            if self.out.send(TunnelMsg::Ack { id, written }).await.is_err() { break false; }
                        }
                    }
                    Some(Inbound::Eof) => {
                        writing = false;
                        let _ = writer.shutdown().await;
                    }
                    Some(Inbound::Abort) => break true,
                    Some(Inbound::Close) | None => break false,
                },
            }
            if !reading && !writing { break true; }
        };
        self.forget(id);
        if tell_peer { let _ = self.out.send(TunnelMsg::Close { id }).await; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    struct Pair {
        viewer: Tunnels,
        host: Tunnels,
        allowed: Arc<AtomicBool>,
        events: mpsc::UnboundedReceiver<TunnelEvent>,
    }

    /// A viewer and a host whose outboxes feed each other's `route`.
    fn pair() -> Pair {
        let (events_tx, events) = mpsc::unbounded_channel();
        let notify: TunnelNotify = Arc::new(move |event| { let _ = events_tx.send(event); });
        let (viewer_out, mut viewer_rx) = mpsc::channel(OUTBOX_CAPACITY);
        let (host_out, mut host_rx) = mpsc::channel(OUTBOX_CAPACITY);
        let allowed = Arc::new(AtomicBool::new(true));
        let viewer = Tunnels::viewer(viewer_out, Arc::clone(&notify));
        let host = Tunnels::host(host_out, Arc::clone(&allowed), notify);
        let to_host = host.clone();
        tokio::spawn(async move { while let Some(message) = viewer_rx.recv().await { to_host.route(message); } });
        let to_viewer = viewer.clone();
        tokio::spawn(async move { while let Some(message) = host_rx.recv().await { to_viewer.route(message); } });
        Pair { viewer, host, allowed, events }
    }

    fn loopback() -> SocketAddr { "127.0.0.1:0".parse().unwrap() }

    /// A server that echoes every connection back.
    async fn echo_server() -> u16 {
        let listener = TcpListener::bind(loopback()).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let (mut reader, mut writer) = socket.split();
                    let _ = tokio::io::copy(&mut reader, &mut writer).await;
                });
            }
        });
        port
    }

    async fn next_event(events: &mut mpsc::UnboundedReceiver<TunnelEvent>, wanted: impl Fn(&TunnelEvent) -> bool) -> TunnelEvent {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let event = events.recv().await.expect("events end");
                if wanted(&event) { return event; }
            }
        })
        .await
        .expect("event in time")
    }

    #[tokio::test]
    async fn carries_more_than_a_window_both_ways() {
        let mut pair = pair();
        let target = echo_server().await;
        let forward = pair.viewer.forward(loopback(), "127.0.0.1", target).await.unwrap();
        let client = TcpStream::connect(("127.0.0.1", forward.local_port)).await.unwrap();
        let (mut reader, mut writer) = client.into_split();
        let payload: Vec<u8> = (0..3 * TUNNEL_WINDOW as usize + 12_345).map(|index| (index % 251) as u8).collect();
        let sent = payload.clone();
        let writing = tokio::spawn(async move { writer.write_all(&sent).await.unwrap(); writer });
        let mut echoed = vec![0u8; payload.len()];
        tokio::time::timeout(Duration::from_secs(20), reader.read_exact(&mut echoed)).await.expect("echo in time").unwrap();
        assert!(echoed == payload, "the echo matches what was sent");
        let _writer = writing.await.unwrap();
        assert_eq!(next_event(&mut pair.events, |event| matches!(event, TunnelEvent::Connected { .. })).await, TunnelEvent::Connected { target: format!("127.0.0.1:{target}") });
        assert_eq!(pair.viewer.forwards()[0].connections, 1);
    }

    #[tokio::test]
    async fn a_half_closed_request_still_gets_its_answer() {
        let pair = pair();
        // Reads the whole request, then answers and closes, like HTTP/1.0.
        let listener = TcpListener::bind(loopback()).await.unwrap();
        let target = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            socket.read_to_end(&mut request).await.unwrap();
            socket.write_all(format!("got {} bytes", request.len()).as_bytes()).await.unwrap();
        });
        let forward = pair.viewer.forward(loopback(), "localhost", target).await.unwrap();
        let mut client = TcpStream::connect(("127.0.0.1", forward.local_port)).await.unwrap();
        client.write_all(&[7; 100_000]).await.unwrap();
        client.shutdown().await.unwrap();
        let mut answer = String::new();
        tokio::time::timeout(Duration::from_secs(10), client.read_to_string(&mut answer)).await.expect("answer in time").unwrap();
        assert_eq!(answer, "got 100000 bytes");
        // Both sides forget the finished connection.
        tokio::time::timeout(Duration::from_secs(5), async {
            while !(locked(&pair.viewer.0.conns).is_empty() && locked(&pair.host.0.conns).is_empty()) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("connections forgotten");
    }

    #[tokio::test]
    async fn a_host_with_tunnels_off_refuses() {
        let mut pair = pair();
        pair.allowed.store(false, Ordering::Relaxed);
        let target = echo_server().await;
        let forward = pair.viewer.forward(loopback(), "127.0.0.1", target).await.unwrap();
        let mut client = TcpStream::connect(("127.0.0.1", forward.local_port)).await.unwrap();
        let refused = next_event(&mut pair.events, |event| matches!(event, TunnelEvent::ForwardRefused { .. })).await;
        assert_eq!(refused, TunnelEvent::ForwardRefused { local_port: forward.local_port, code: TUNNEL_NOT_ALLOWED.into() });
        let mut byte = [0u8; 1];
        assert_eq!(tokio::time::timeout(Duration::from_secs(5), client.read(&mut byte)).await.expect("closed in time").unwrap_or(0), 0);
    }

    #[tokio::test]
    async fn an_unreachable_target_is_refused() {
        let mut pair = pair();
        // A port that was free a moment ago: nothing listens there.
        let target = TcpListener::bind(loopback()).await.unwrap().local_addr().unwrap().port();
        let forward = pair.viewer.forward(loopback(), "127.0.0.1", target).await.unwrap();
        let _client = TcpStream::connect(("127.0.0.1", forward.local_port)).await.unwrap();
        let refused = next_event(&mut pair.events, |event| matches!(event, TunnelEvent::ForwardRefused { .. })).await;
        assert_eq!(refused, TunnelEvent::ForwardRefused { local_port: forward.local_port, code: TUNNEL_UNREACHABLE.into() });
    }

    #[tokio::test]
    async fn turning_tunnels_off_closes_open_connections() {
        let pair = pair();
        let target = echo_server().await;
        let forward = pair.viewer.forward(loopback(), "127.0.0.1", target).await.unwrap();
        let mut client = TcpStream::connect(("127.0.0.1", forward.local_port)).await.unwrap();
        client.write_all(b"ping").await.unwrap();
        let mut echoed = [0u8; 4];
        client.read_exact(&mut echoed).await.unwrap();
        pair.allowed.store(false, Ordering::Relaxed);
        pair.host.close_all();
        let mut rest = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), client.read_to_end(&mut rest)).await.expect("closed in time").ok();
        assert!(rest.is_empty());
    }

    #[tokio::test]
    async fn stopping_a_forward_frees_its_port() {
        let pair = pair();
        let target = echo_server().await;
        let forward = pair.viewer.forward(loopback(), "127.0.0.1", target).await.unwrap();
        assert_eq!(pair.viewer.forwards().len(), 1);
        assert!(pair.viewer.stop_forward(forward.id));
        assert!(pair.viewer.forwards().is_empty());
        assert!(!pair.viewer.stop_forward(forward.id));
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if TcpListener::bind(("127.0.0.1", forward.local_port)).await.is_ok() { break; }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("port free again");
    }

    #[tokio::test]
    async fn a_peer_ignoring_the_window_loses_the_connection() {
        let pair = pair();
        let target = echo_server().await;
        let forward = pair.viewer.forward(loopback(), "127.0.0.1", target).await.unwrap();
        let mut client = TcpStream::connect(("127.0.0.1", forward.local_port)).await.unwrap();
        client.write_all(b"hi").await.unwrap();
        let mut echoed = [0u8; 2];
        client.read_exact(&mut echoed).await.unwrap();
        let id = *locked(&pair.viewer.0.conns).keys().next().unwrap();
        // Our client stops reading while a "host" pushes far past the window.
        for _ in 0..=(TUNNEL_WINDOW as usize / MAX_TUNNEL_CHUNK) + 1 {
            pair.viewer.route(TunnelMsg::Data { id, data: vec![0; MAX_TUNNEL_CHUNK] });
        }
        tokio::time::timeout(Duration::from_secs(5), async {
            while locked(&pair.viewer.0.conns).contains_key(&id) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("connection dropped");
    }

    #[tokio::test]
    async fn the_viewer_learns_the_host_offers_tunnels() {
        let mut pair = pair();
        assert!(!pair.viewer.available());
        assert!(pair.host.announce().await);
        assert!(!pair.viewer.announce().await);
        assert_eq!(next_event(&mut pair.events, |_| true).await, TunnelEvent::Available);
        assert!(pair.viewer.available());
        assert!(!pair.host.available());
    }

    #[test]
    fn targets_are_checked_and_displayed() {
        assert_eq!(target_host(" printer.local "), Some("printer.local"));
        assert_eq!(target_host("[::1]"), Some("::1"));
        assert_eq!(target_host("fe80::1%eth0"), Some("fe80::1%eth0"));
        assert_eq!(target_host(""), None);
        assert_eq!(target_host("evil host"), None);
        assert_eq!(target_host("a/b"), None);
        assert_eq!(target_host(&"a".repeat(MAX_TUNNEL_HOST + 1)), None);
        assert_eq!(display_target("::1", 22), "[::1]:22");
        assert_eq!(display_target("10.0.0.2", 3389), "10.0.0.2:3389");
    }

    #[tokio::test]
    async fn forward_rejects_bad_targets() {
        let pair = pair();
        assert_eq!(pair.viewer.forward(loopback(), "", 22).await.unwrap_err().kind(), io::ErrorKind::InvalidInput);
        assert_eq!(pair.viewer.forward(loopback(), "localhost", 0).await.unwrap_err().kind(), io::ErrorKind::InvalidInput);
        assert_eq!(pair.host.forward(loopback(), "localhost", 22).await.unwrap_err().kind(), io::ErrorKind::Unsupported);
    }
}
