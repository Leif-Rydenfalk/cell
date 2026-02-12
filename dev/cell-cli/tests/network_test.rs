use anyhow::Result;
use cell_cli::golgi::{Golgi, Target};
use cell_cli::{antigens, synapse};
use serial_test::serial;
use std::collections::HashMap;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UnixListener};

/// Simulates a running "Worker" process (Nucleus).
/// Acts as a Server listening on its own socket.
async fn spawn_mock_nucleus(
    socket_path: std::path::PathBuf,
    _service_name: &str,
    schema_content: &str,
) {
    // 1. Bind the Service Socket
    if socket_path.exists() {
        std::fs::remove_file(&socket_path).expect("Failed to clean mock socket");
    }
    let listener =
        UnixListener::bind(&socket_path).expect("Mock Nucleus failed to bind Service Socket");

    // 2. Accept Connection from Golgi
    loop {
        match listener.accept().await {
            Ok((mut stream, _)) => {
                let schema = schema_content.to_string();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    loop {
                        // Read Length
                        let mut len_buf = [0u8; 4];
                        if stream.read_exact(&mut len_buf).await.is_err() {
                            break;
                        }
                        let len = u32::from_be_bytes(len_buf) as usize;

                        // Read Body
                        if stream.read_exact(&mut buf[..len]).await.is_err() {
                            break;
                        }
                        let msg = &buf[..len];

                        // Logic: Handle __GENOME__ request
                        if msg == b"__GENOME__" {
                            let resp = schema.as_bytes();
                            let rlen = (resp.len() as u32).to_be_bytes();
                            stream.write_all(&rlen).await.unwrap();
                            stream.write_all(resp).await.unwrap();
                        } else {
                            // Echo
                            let rlen = (len as u32).to_be_bytes();
                            stream.write_all(&rlen).await.unwrap();
                            stream.write_all(msg).await.unwrap();
                        }
                    }
                });
            }
            Err(_) => break,
        }
    }
}

#[tokio::test]
#[serial]
async fn test_full_handshake_and_routing() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let run_dir = temp.path().join("run");
    std::fs::create_dir_all(&run_dir)?;

    let port = 9091;
    let axon_addr = format!("127.0.0.1:{}", port);
    let mut routes = HashMap::new();

    let worker_sock = run_dir.join("worker.sock");
    routes.insert(
        "worker".to_string(),
        Target::GapJunction(worker_sock.clone()),
    );

    // 1. Spawn Mock Nucleus FIRST
    let schema = r#"{ "input": "Job", "output": "Result" }"#;
    let sock_path = worker_sock.clone();
    tokio::spawn(async move {
        spawn_mock_nucleus(sock_path, "worker", schema).await;
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    // 2. Start Golgi
    let golgi = Golgi::new(
        "router".to_string(),
        &run_dir,
        Some(axon_addr.clone()),
        routes,
        false,
    )?;

    tokio::spawn(async move {
        golgi.run().await.unwrap();
    });

    tokio::time::sleep(Duration::from_millis(200)).await;

    // 3. Client Logic
    let client_id_path = run_dir.join("client_id");
    let client_identity = antigens::Antigens::load_or_create(client_id_path)?;

    let tcp_stream = TcpStream::connect(&axon_addr).await?;

    let (mut secure_stream, _) =
        synapse::connect_secure(tcp_stream, &client_identity.keypair, true).await?;
    println!("✅ Handshake Complete");

    let target = "worker";
    let mut buf = vec![0u8; 1024];

    let mut connect_frame = vec![0x01];
    connect_frame.extend(&(target.len() as u32).to_be_bytes());
    connect_frame.extend(target.as_bytes());

    let len = secure_stream
        .state
        .write_message(&connect_frame, &mut buf)
        .unwrap();
    synapse::write_frame(&mut secure_stream.inner, &buf[..len]).await?;

    // Read ACK
    let frame = synapse::read_frame(&mut secure_stream.inner).await?;
    let _len = secure_stream.state.read_message(&frame, &mut buf)?;

    assert_eq!(
        buf[0], 0x00,
        "Expected ACK (0x00), got NACK (0xFF). Is the mock nucleus running?"
    );
    println!("Route Established");

    // Fetch Genome
    let req = b"__GENOME__";
    let mut vesicle = (req.len() as u32).to_be_bytes().to_vec();
    vesicle.extend_from_slice(req);

    let len = secure_stream
        .state
        .write_message(&vesicle, &mut buf)
        .unwrap();
    synapse::write_frame(&mut secure_stream.inner, &buf[..len]).await?;

    let frame = synapse::read_frame(&mut secure_stream.inner).await?;
    let _len = secure_stream.state.read_message(&frame, &mut buf)?;

    let resp_len = u32::from_be_bytes(buf[0..4].try_into().unwrap()) as usize;
    let json_resp = String::from_utf8(buf[4..4 + resp_len].to_vec())?;

    assert_eq!(json_resp, schema);
    println!("Schema Fetched: {}", json_resp);

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_route_not_found() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let run_dir = temp.path().join("run");
    std::fs::create_dir_all(&run_dir)?;

    let port = 9092;
    let axon_addr = format!("127.0.0.1:{}", port);

    let golgi = Golgi::new(
        "router".to_string(),
        &run_dir,
        Some(axon_addr.clone()),
        HashMap::new(),
        false,
    )?;

    tokio::spawn(async move {
        golgi.run().await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(200)).await;

    let client_id_path = run_dir.join("client_id");
    let client_identity = antigens::Antigens::load_or_create(client_id_path)?;

    let tcp_stream = TcpStream::connect(&axon_addr).await?;
    let (mut secure_stream, _) =
        synapse::connect_secure(tcp_stream, &client_identity.keypair, true).await?;

    let target = "fake_service";
    let mut buf = vec![0u8; 1024];
    let mut connect_frame = vec![0x01];
    connect_frame.extend(&(target.len() as u32).to_be_bytes());
    connect_frame.extend(target.as_bytes());

    let len = secure_stream
        .state
        .write_message(&connect_frame, &mut buf)
        .unwrap();
    synapse::write_frame(&mut secure_stream.inner, &buf[..len]).await?;

    let frame = synapse::read_frame(&mut secure_stream.inner).await?;
    let _len = secure_stream.state.read_message(&frame, &mut buf)?;

    assert_eq!(buf[0], 0xFF);
    Ok(())
}
