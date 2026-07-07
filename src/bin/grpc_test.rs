//! Quick gRPC connectivity test. Not part of the main binary.
//! Usage: YELLOWSTONE_GRPC_TOKEN="your_token" cargo run --release --bin grpc_test

use yellowstone_grpc_client::GeyserGrpcClient;

const GRPC_ENDPOINT: &str = "https://grpc.example.com";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let token = std::env::var("YELLOWSTONE_GRPC_TOKEN")
        .unwrap_or_else(|_| "".to_string());
    let mut client = GeyserGrpcClient::build_from_shared(GRPC_ENDPOINT)?
        .x_token(if token.is_empty() { None } else { Some(token) })?
        .connect()
        .await?;
    let v = client.get_version().await?;
    println!("OK: {:?}", v);
    Ok(())
}
