use anyhow::Result;
use cell_cli::golgi::{Golgi, Target};
use serial_test::serial;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

async fn spawn_mock_worker(path: std::path::PathBuf, id: u8) {
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).unwrap();

    tokio::spawn(async move {
        loop {
            if let Ok((mut stream, _)) = listener.accept().await {
                // Read Op
                let mut buf = [0u8; 1];
                let _ = stream.read_exact(&mut buf).await;
                // Echo ID back
                let _ = stream.write_all(&[id]).await;
            }
        }
    });
}

#[tokio::test]
#[serial]
async fn test_golgi_round_robin_routing() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let run_dir = temp.path().join("run");
    std::fs::create_dir_all(&run_dir)?;

    // 1. Setup 3 Mock Workers
    let mut sockets = Vec::new();
    for i in 0..3 {
        let sock = run_dir.join(format!("worker_{}.sock", i));
        spawn_mock_worker(sock.clone(), i as u8).await;
        sockets.push(sock);
    }

    // 2. Configure Golgi Route
    let mut routes = HashMap::new();
    routes.insert("colony".to_string(), Target::LocalColony(Arc::new(sockets)));

    let golgi = Golgi::new(
        "router".to_string(),
        &run_dir,
        None, // No TCP needed
        routes,
        false,
    )?;

    tokio::spawn(async move {
        golgi.run().await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    // 3. Connect to Golgi and send requests to "colony"
    let golgi_sock = run_dir.join("golgi.sock");

    let mut responses = Vec::new();
    for _ in 0..6 {
        let mut stream = UnixStream::connect(&golgi_sock).await?;

        // Handshake: Op 0x01 (Connect) | Len: 6 | "colony"
        stream.write_all(&[0x01]).await?;
        let name = b"colony";
        stream.write_all(&(name.len() as u32).to_be_bytes()).await?;
        stream.write_all(name).await?;

        // Read ACK
        let mut ack = [0u8; 1];
        stream.read_exact(&mut ack).await?;
        assert_eq!(ack[0], 0x00, "Golgi NACKed");

        // Read Worker Response (Mock returns its ID)
        let mut resp = [0u8; 1];
        stream.read_exact(&mut resp).await?;
        responses.push(resp[0]);
    }

    // 4. Verify Round Robin Distribution
    // Expected pattern: 0, 1, 2, 0, 1, 2 (or similar depending on starting atomic index)
    println!("Responses: {:?}", responses);

    assert_eq!(responses.len(), 6);
    // Check if we hit all 3 workers
    assert!(responses.contains(&0));
    assert!(responses.contains(&1));
    assert!(responses.contains(&2));

    Ok(())
}
