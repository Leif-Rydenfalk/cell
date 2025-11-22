use anyhow::Result;
use cell_sdk::*;
use rand::Rng;
use std::time::Duration;
use tokio::time::sleep;

// Define the Protocol
signal_receptor! {
    name: chatterbox,
    input: Gossip {
        from_pid: u32,
        content: String,
    },
    output: Ack {
        status: String,
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let my_pid = std::process::id();
    
    // 1. BACKGROUND TALKER (Client)
    // Every worker is also a client ("Mesh" behavior)
    tokio::spawn(async move {
        // Wait for system to boot
        sleep(Duration::from_secs(5)).await; 

        loop {
            // Sleep random amount (100ms to 2000ms) to create chaotic traffic
            let delay = rand::thread_rng().gen_range(100..2000);
            sleep(Duration::from_millis(delay)).await;

            let msg = Gossip {
                from_pid: my_pid,
                content: format!("Hello from process {}", my_pid),
            };

            // Call the colony. 
            // Your Golgi router will Round-Robin this to a random peer (or self).
            match call_as!(chatterbox, msg) {
                Ok(ack) => {
                    // Success (Silent to keep logs clean, or uncomment to see acks)
                    // println!("Sent gossip, got: {}", ack.status);
                }
                Err(e) => {
                    println!("Failed to gossip: {}", e);
                }
            }
        }
    });

    // 2. LISTENER (Server)
    println!("Chatterbox Node {} Online.", my_pid);

    Membrane::bind(__GENOME__, move |vesicle| {
        let msg = cell_sdk::rkyv::check_archived_root::<Gossip>(vesicle.as_slice())
            .map_err(|e| anyhow::anyhow!("Bad Data: {}", e))?;

        let my_pid = std::process::id();

        // LOGGING: This goes to stdout -> Pipe -> Vacuole -> service.log
        // We will see who talked to whom.
        println!("Recv: [{} says '{}']", msg.from_pid, msg.content);

        let resp = Ack {
            status: format!("Ack from {}", my_pid),
        };

        let bytes = cell_sdk::rkyv::to_bytes::<_, 256>(&resp)?.into_vec();
        Ok(vesicle::Vesicle::wrap(bytes))
    })
}