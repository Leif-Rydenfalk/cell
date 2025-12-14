// client/src/main.rs
use cell_sdk::cell_remote;

cell_remote!(Hello = "hello");

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut client = Hello::Client::connect().await?;
    
    let resp = client.ping("World".to_string()).await?;
    
    println!("✅ Response: {}", resp);
    
    Ok(())
}