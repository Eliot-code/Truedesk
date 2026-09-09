use tokio::net::TcpListener;

#[tokio::main]
async fn main() {
    let listener = TcpListener::bind("0.0.0.0:8080").await.unwrap();
    println!("relay listening on {}", listener.local_addr().unwrap());
    axum::serve(listener, relay::app()).await.unwrap();
}
