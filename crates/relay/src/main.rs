use tokio::net::TcpListener;
use tokio::sync::watch;

#[tokio::main]
async fn main() {
    // PORT is injected by the host platform (e.g. Coolify); default 8080.
    let port = std::env::var("PORT").unwrap_or_else(|_| "8080".to_string());
    let addr = format!("0.0.0.0:{port}");
    let listener = TcpListener::bind(&addr).await.unwrap();
    println!("relay listening on {}", listener.local_addr().unwrap());
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    axum::serve(listener, relay::app_with_shutdown(shutdown_rx).into_make_service_with_connect_info::<std::net::SocketAddr>())
        .with_graceful_shutdown(shutdown_signal(shutdown_tx))
        .await
        .unwrap();
}

async fn shutdown_signal(shutdown_tx: watch::Sender<bool>) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
        tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = term.recv() => {} }
    }
    #[cfg(not(unix))]
    { tokio::signal::ctrl_c().await.expect("install ctrl-c handler"); }
    let _ = shutdown_tx.send(true);
}
