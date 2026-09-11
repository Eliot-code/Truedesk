use tokio::net::TcpListener;

#[tokio::main]
async fn main() {
    // PORT is injected by the host platform (e.g. Coolify); default 8080.
    let port = std::env::var("PORT").unwrap_or_else(|_| "8080".to_string());
    let addr = format!("0.0.0.0:{port}");
    let listener = TcpListener::bind(&addr).await.unwrap();
    println!("relay listening on {}", listener.local_addr().unwrap());
    axum::serve(listener, relay::app()).await.unwrap();
}
