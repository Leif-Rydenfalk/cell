use anyhow::{Context, Result};
use cell_sdk::Synapse;
use tokio::net::UnixListener;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_target(false).init();
    println!("--- ROUTER ONLINE ---");

    // 1. Setup Input (Membrane) manually
    // We don't use the #[service] macro because we are doing raw frame forwarding,
    // not typed deserialization. We operate at Layer 4 (Transport).
    let cwd = std::env::current_dir()?;
    let io_dir = cwd.join(".cell/io");
    std::fs::create_dir_all(&io_dir)?;
    
    let rx_path = io_dir.join("in");
    if rx_path.exists() { std::fs::remove_file(&rx_path)?; }
    let listener = UnixListener::bind(&rx_path).context("Failed to bind router socket")?;

    // 2. Main Loop
    loop {
        let (mut client_stream, _) = listener.accept().await?;
        
        // Spawn per connection
        tokio::spawn(async move {
            if let Err(e) = handle_proxy(client_stream).await {
                eprintln!("Proxy Error: {}", e);
            }
        });
    }
}

async fn handle_proxy(mut client_stream: tokio::net::UnixStream) -> Result<()> {
    // A. Connect to Destination (Exchange)
    // In a real router, we'd read the TargetID from the frame header to decide where to go.
    // For this bench, we statically route everything to "exchange".
    let mut downstream = Synapse::grow("exchange").await?;

    loop {
        // 1. Read Frame Header (Length) from Client
        let mut len_buf = [0u8; 4];
        if client_stream.read_exact(&mut len_buf).await.is_err() {
            break; // Client disconnected
        }
        let len = u32::from_le_bytes(len_buf) as usize;

        // 2. Read Frame Body
        let mut buf = vec![0u8; len];
        client_stream.read_exact(&mut buf).await?;

        // 3. Forward to Exchange
        // We split the frame because Synapse::fire_on_channel expects payload + channel
        // Frame = [Header:24] [Channel:1] [Payload...]
        if buf.len() < 25 { continue; }
        
        let channel = buf[24];
        let payload = &buf[25..];

        // "Fire" handles sending and waiting for the reply
        let response = downstream.fire_on_channel(channel, payload).await?;
        let resp_bytes = response.into_owned();

        // 4. Send Reply back to Client
        let total_len = resp_bytes.len();
        client_stream.write_all(&(total_len as u32).to_le_bytes()).await?;
        client_stream.write_all(&resp_bytes).await?;
    }
    Ok(())
}