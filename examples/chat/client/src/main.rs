// chat/client/src/main.rs
use anyhow::Result;
use cell_sdk::cell_remote;
use std::io::Write;

cell_remote!(Chat = "chat");

#[tokio::main]
async fn main() -> Result<()> {
    // 1. Parse Args (Username)
    let args: Vec<String> = std::env::args().collect();
    let user = args.get(1).cloned().unwrap_or_else(|| {
        print!("Enter username: ");
        std::io::stdout().flush().unwrap();
        let mut s = String::new();
        std::io::stdin().read_line(&mut s).unwrap();
        s.trim().to_string()
    });

    println!("Connecting to Chat Mesh as '{}'...", user);

    // 2. Connect
    let mut client = Chat::Client::connect().await?;
    println!("✅ Connected. Type a message and press Enter.");

    // 3. Spawn Listener (Polling for demo simplicity, real app would use streaming)
    let mut poll_client = Chat::Client::connect().await?; // Separate connection for polling
    tokio::spawn(async move {
        let mut last_timestamp = 0;
        loop {
            // Poll for messages since last_timestamp
            match poll_client.stream(last_timestamp).await {
                Ok(messages) => {
                    for msg in messages {
                        if msg.timestamp > last_timestamp {
                            last_timestamp = msg.timestamp;
                            println!("\n[{}] {}: {}", msg.timestamp, msg.user, msg.text);
                            print!("> ");
                            std::io::stdout().flush().unwrap();
                        }
                    }
                }
                Err(_) => { /* Retry silently */ }
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    });

    // 4. Input Loop
    loop {
        print!("> ");
        std::io::stdout().flush()?;
        
        let mut text = String::new();
        std::io::stdin().read_line(&mut text)?;
        let text = text.trim();
        
        if text.is_empty() { continue; }
        if text == "/quit" { break; }

        let req = Chat::SendRequest {
            user: user.clone(),
            text: text.to_string(),
        };

        if let Err(e) = client.send(req).await {
            eprintln!("Failed to send: {}", e);
        }
    }

    Ok(())
}