pub mod discovery;

use crate::transport;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::RwLock;

pub struct Golgi {
    name: String,
    run_dir: PathBuf,
    quic_port: u16,
    lighthouse_addr: Option<String>,
    peers: Arc<RwLock<HashMap<String, SocketAddr>>>,
}

impl Golgi {
    pub fn new(
        name: String,
        run_dir: PathBuf,
        quic_port: u16,
        lighthouse_addr: Option<String>,
    ) -> Self {
        Self {
            name,
            run_dir,
            quic_port,
            lighthouse_addr,
            peers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn run(self) -> Result<()> {
        let sock_path = self.run_dir.join("golgi.sock");
        if sock_path.exists() {
            tokio::fs::remove_file(&sock_path).await.ok();
        }
        let unix_listener = UnixListener::bind(&sock_path)?;

        // 1. Setup QUIC
        let (cert, key) = transport::generate_cert(vec![self.name.clone(), "localhost".into()])?;
        let server_config = transport::make_server_config((cert, key))?;
        let client_config = transport::make_client_config()?;

        let endpoint = quinn::Endpoint::server(
            server_config,
            format!("0.0.0.0:{}", self.quic_port).parse()?,
        )?;
        println!("[Golgi] QUIC Listening on 0.0.0.0:{}", self.quic_port);

        // 2. Start Discovery (If Lighthouse configured)
        let discovery = if let Some(lh) = &self.lighthouse_addr {
            // Use port+1 for discovery signals
            let d = discovery::Discovery::new(self.quic_port + 1, lh).await?;

            // Heartbeat Loop
            let d_arc = Arc::new(d);
            let d_clone = d_arc.clone();
            let my_name = self.name.clone();
            let my_port = self.quic_port;

            tokio::spawn(async move {
                loop {
                    let _ = d_clone.register(my_name.clone(), my_port).await;
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                }
            });
            println!("[Golgi] Connected to Lighthouse: {}", lh);
            Some(d_arc)
        } else {
            None
        };

        let peers_map = self.peers.clone();
        let endpoint_clone = endpoint.clone();

        // 3. Main Loop
        loop {
            tokio::select! {
                // Local IPC Request
                Ok((stream, _)) = unix_listener.accept() => {
                    let ep = endpoint.clone();
                    let cc = client_config.clone();
                    let disc = discovery.clone();
                    let pm = peers_map.clone();
                    tokio::spawn(async move {
                        handle_local_request(stream, ep, cc, disc, pm).await;
                    });
                }

                // Incoming QUIC Connection
                Some(conn) = endpoint_clone.accept() => {
                    tokio::spawn(async move {
                        if let Ok(connection) = conn.await {
                            handle_remote_connection(connection).await;
                        }
                    });
                }
            }
        }
    }
}

async fn handle_local_request(
    mut stream: UnixStream,
    endpoint: quinn::Endpoint,
    client_config: quinn::ClientConfig,
    discovery: Option<Arc<discovery::Discovery>>,
    peers: Arc<RwLock<HashMap<String, SocketAddr>>>,
) {
    // 1. Read Request Header
    // [Op: u8] [Len: u32] [TargetName: Bytes]
    let op = match stream.read_u8().await {
        Ok(b) => b,
        Err(_) => return,
    };
    let name_len = match stream.read_u32().await {
        Ok(n) => n,
        Err(_) => return,
    };
    let mut name_buf = vec![0u8; name_len as usize];
    if stream.read_exact(&mut name_buf).await.is_err() {
        return;
    }
    let target = String::from_utf8_lossy(&name_buf).to_string();

    if op != 0x01 {
        return;
    } // Only handle CONNECT for now

    // 2. Resolve Address
    let addr = {
        let map = peers.read().await;
        map.get(&target).cloned()
    };

    let remote_addr = match addr {
        Some(a) => a,
        None => {
            // Try Lighthouse
            let mut found = None;
            if let Some(d) = discovery {
                if let Ok(Some(a)) = d.lookup(target.clone()).await {
                    peers.write().await.insert(target.clone(), a);
                    found = Some(a);
                }
            }

            match found {
                Some(a) => a,
                None => {
                    let _ = stream.write_u8(0xFF).await; // NACK
                    return;
                }
            }
        }
    };

    // 3. Connect via QUIC
    let connection = match endpoint.connect_with(client_config, remote_addr, "localhost") {
        Ok(connecting) => match connecting.await {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Connect failed: {}", e);
                let _ = stream.write_u8(0xFF).await;
                return;
            }
        },
        Err(_) => {
            let _ = stream.write_u8(0xFF).await;
            return;
        }
    };

    // 4. Open Bidirectional Stream
    let (mut send, mut recv) = match connection.open_bi().await {
        Ok(s) => s,
        Err(_) => {
            let _ = stream.write_u8(0xFF).await;
            return;
        }
    };

    // 5. Bridge Loop
    let _ = stream.write_u8(0x00).await; // ACK

    // Send the target name to the remote Golgi so it knows where to route locally
    let _ = send.write_u32(target.len() as u32).await;
    let _ = send.write_all(target.as_bytes()).await;

    let (mut rs, mut ws) = tokio::io::split(stream);

    let _ = tokio::join!(
        async {
            // Local Unix -> Remote QUIC
            let mut buf = [0u8; 4096];
            loop {
                match rs.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        if send.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = send.finish().await;
        },
        async {
            // Remote QUIC -> Local Unix
            let mut buf = [0u8; 4096];
            loop {
                match recv.read(&mut buf).await {
                    Ok(Some(n)) => {
                        if ws.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
        }
    );
}

async fn handle_remote_connection(connection: quinn::Connection) {
    while let Ok((mut send, mut recv)) = connection.accept_bi().await {
        tokio::spawn(async move {
            // Read Target Name from stream
            let name_len = match recv.read_u32().await {
                Ok(n) => n,
                Err(_) => return,
            };
            let mut name_buf = vec![0u8; name_len as usize];
            if recv.read_exact(&mut name_buf).await.is_err() {
                return;
            }
            // let target = String::from_utf8_lossy(&name_buf);

            // In a full router, we check if 'target' is us or a child.
            // For MVP, we assume the remote wants OUR local cell socket.

            let sock_path =
                std::env::var("CELL_SOCKET_PATH").unwrap_or_else(|_| "run/cell.sock".to_string());

            if let Ok(mut local_stream) = UnixStream::connect(sock_path).await {
                let (mut ls_r, mut ls_w) = local_stream.into_split();
                let _ = tokio::join!(
                    async {
                        // Remote -> Local
                        let _ = tokio::io::copy(&mut recv, &mut ls_w).await;
                    },
                    async {
                        // Local -> Remote
                        let mut buf = [0u8; 4096];
                        loop {
                            match ls_r.read(&mut buf).await {
                                Ok(0) => break,
                                Ok(n) => {
                                    if send.write_all(&buf[..n]).await.is_err() {
                                        break;
                                    }
                                }
                                Err(_) => break,
                            }
                        }
                        let _ = send.finish().await;
                    }
                );
            }
        });
    }
}
