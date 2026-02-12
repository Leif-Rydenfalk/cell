use anyhow::Result;
use cell_cli::golgi::Golgi;
use cell_cli::synapse;
use std::collections::HashMap;
use tokio::net::TcpStream;

#[tokio::test]
async fn test_secure_handshake() -> Result<()> {
    // 1. Setup Server (Golgi)
    let temp_dir = tempfile::tempdir()?;
    let run_dir = temp_dir.path().join("run");
    std::fs::create_dir_all(&run_dir)?;

    let server_addr = "127.0.0.1:9095";
    let routes = HashMap::new();

    let golgi = Golgi::new(
        "router".to_string(),
        &run_dir,
        Some(server_addr.to_string()),
        routes,
        false,
    )?;

    tokio::spawn(async move {
        golgi.run().await.unwrap();
    });

    // Give it a moment to bind
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // 2. Client Side: Connect
    let client_id_path = run_dir.join("client_id");
    let client_identity = cell_cli::antigens::Antigens::load_or_create(client_id_path)?;

    let stream = TcpStream::connect(server_addr).await?;

    // 3. Perform Handshake
    let (mut secure_stream, remote_pub) =
        synapse::connect_secure(stream, &client_identity.keypair, true).await?;

    println!("Connected to remote node: {:?}", remote_pub);
    assert!(!remote_pub.is_empty());

    // 4. Send a "Connect" Frame to a non-existent service
    let mut buf = vec![0u8; 1024];
    let target_name = "ghost_service";

    // Payload: [0x01] [Len] [Name]
    let mut payload = vec![0x01];
    payload.extend(&(target_name.len() as u32).to_be_bytes());
    payload.extend(target_name.as_bytes());

    let len = secure_stream
        .state
        .write_message(&payload, &mut buf)
        .unwrap();
    synapse::write_frame(&mut secure_stream.inner, &buf[..len]).await?;

    // 5. Read Response
    let frame = synapse::read_frame(&mut secure_stream.inner).await?;
    let _len = secure_stream.state.read_message(&frame, &mut buf).unwrap();

    // Expect NACK (0xFF) because "ghost_service" doesn't exist
    assert_eq!(buf[0], 0xFF);

    Ok(())
}
