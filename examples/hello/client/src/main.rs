// client/src/main.rs
use cell_sdk::cell_remote;
use anyhow::Result;

cell_remote!(Hello = "hello");

#[tokio::main]
async fn main() -> Result<()> {
    let mut client = Hello::Client::connect().await?;
    
    let resp = client.ping(Hello::Ping { msg: "World".to_string() }).await?;
    
    println!("Response: {}", resp);
    
    Ok(())
}