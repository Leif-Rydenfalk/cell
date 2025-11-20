use crate::antigens::Antigens;
use crate::synapse;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UnixListener, UnixStream};

// --- Types ---

#[derive(Clone, Debug)]
pub enum Target {
    GapJunction(PathBuf), // Local Service (Unix Socket)
    Axon(String),         // Remote Node (TCP Address)
}

pub struct Golgi {
    socket_path: PathBuf,
    axon_bind: Option<String>,
    routes: Arc<HashMap<String, Target>>,
    identity: Arc<Antigens>,
}

// --- Logging Helper ---

fn sys_log(level: &str, msg: &str) {
    let timestamp = humantime::format_rfc3339_seconds(SystemTime::now());
    eprintln!("[{}] [{}] [GOLGI] {}", timestamp, level, msg);
}

// --- Implementation ---

impl Golgi {
    pub fn new(
        run_dir: &std::path::Path,
        axon_bind: Option<String>,
        routes_map: HashMap<String, Target>,
    ) -> Result<Self> {
        // Load cryptographic identity
        let identity = Antigens::load_or_create()
            .context("Failed to load node identity. System integrity compromised.")?;

        sys_log("INFO", &format!("Identity Loaded. Node ID: {}", identity.public_key_str));

        Ok(Self {
            socket_path: run_dir.join("golgi.sock"),
            axon_bind,
            routes: Arc::new(routes_map),
            identity: Arc::new(identity),
        })
    }

    /// The main transport loop. This blocks until the process is killed.
    pub async fn run(self) -> Result<()> {
        // 1. Initialize Gap Junction Interface (Local)
        if self.socket_path.exists() {
            tokio::fs::remove_file(&self.socket_path).await.ok();
        }
        let unix_listener = UnixListener::bind(&self.socket_path)
            .context("CRITICAL: Failed to bind internal Gap Junction socket.")?;

        // 2. Initialize Axon Interface (Remote)
        let tcp_listener = if let Some(addr) = &self.axon_bind {
            let l = TcpListener::bind(addr)
                .await
                .context("CRITICAL: Failed to bind external Axon interface.")?;
            sys_log("INFO", &format!("Axon Interface online: {}", addr));
            Some(l)
        } else {
            sys_log("WARN", "No Axon interface configured. Node is isolated.");
            None
        };

        let routes = self.routes;
        let identity = self.identity;

        sys_log("INFO", "Transport subsystem active. Waiting for signals...");

        loop {
            tokio::select! {
                // --- LOCAL INGRESS (Gap Junction) ---
                res = unix_listener.accept() => {
                    match res {
                        Ok((stream, _)) => {
                            let r = routes.clone();
                            let i = identity.clone();
                            tokio::spawn(async move {
                                if let Err(e) = handle_local_signal(stream, r, i).await {
                                    sys_log("ERROR", &format!("Local signal processing failed: {}", e));
                                }
                            });
                        }
                        Err(e) => sys_log("ERROR", &format!("Gap Junction accept failed: {}", e)),
                    }
                }

                // --- REMOTE INGRESS (Axon) ---
                res = accept_tcp_optional(tcp_listener.as_ref()) => {
                    if let Some(Ok((stream, addr))) = res {
                        let r = routes.clone();
                        let i = identity.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_remote_signal(stream, r, i).await {
                                sys_log("WARN", &format!("Remote signal rejected from {}: {}", addr, e));
                            }
                        });
                    }
                }
            }
        }
    }
}

// --- Handlers ---

/// Handles traffic originating from the Local Node.
/// This traffic is trusted plaintext. It may be destined for local or remote targets.
async fn handle_local_signal(
    mut stream: UnixStream,
    routes: Arc<HashMap<String, Target>>,
    identity: Arc<Antigens>,
) -> Result<()> {
    // 1. Read Protocol OpCode
    let mut op = [0u8; 1];
    if stream.read_exact(&mut op).await.is_err() {
        return Ok(()); // Connection closed immediately
    }

    if op[0] == 0x01 {
        // CONNECT REQUEST
        let target_name = read_length_prefixed_string(&mut stream).await?;

        match routes.get(&target_name) {
            Some(Target::GapJunction(path)) => {
                // Local -> Local routing
                match UnixStream::connect(path).await {
                    Ok(target) => {
                        stream.write_all(&[0x00]).await?; // ACK
                        bridge_streams(stream, target).await?;
                    }
                    Err(e) => {
                        sys_log("WARN", &format!("Target service '{}' unavailable: {}", target_name, e));
                        stream.write_all(&[0xFF]).await?; // NACK
                    }
                }
            }
            Some(Target::Axon(addr)) => {
                // Local -> Remote routing
                // We must act as the Client (Initiator)
                let tcp_stream = TcpStream::connect(addr).await.context("Remote node unreachable")?;
                tcp_stream.set_nodelay(true)?;

                // Perform Encrypted Handshake
                let (mut secure_stream, remote_pub) =
                    synapse::connect_secure(tcp_stream, &identity.keypair, true).await?;
                
                let remote_id = base64::encode(remote_pub);
                // sys_log("DEBUG", &format!("Established secure link to {}", remote_id));

                // Forward the Request inside the tunnel
                secure_stream.write_all(&[0x01]).await?; // OpCode
                write_length_prefixed_string(&mut secure_stream, &target_name).await?;

                // Wait for Remote ACK
                let mut ack = [0u8; 1];
                secure_stream.read_exact(&mut ack).await?;

                if ack[0] == 0x00 {
                    stream.write_all(&[0x00]).await?; // ACK to local
                    bridge_streams(stream, secure_stream).await?;
                } else {
                    stream.write_all(&[0xFF]).await?; // NACK to local
                }
            }
            None => {
                sys_log("WARN", &format!("Route not found: {}", target_name));
                stream.write_all(&[0xFF]).await?; // NACK
            }
        }
    }
    Ok(())
}

/// Handles traffic originating from a Remote Node.
/// This traffic is untrusted and must be authenticated/decrypted.
async fn handle_remote_signal(
    stream: TcpStream,
    routes: Arc<HashMap<String, Target>>,
    identity: Arc<Antigens>,
) -> Result<()> {
    stream.set_nodelay(true)?;
    
    // 1. Perform Encrypted Handshake (Responder)
    let (mut secure_stream, remote_pub) =
        synapse::connect_secure(stream, &identity.keypair, false).await?;

    let remote_id = base64::encode(remote_pub);
    // In a future update, we would check an Access Control List (ACL) here.
    // sys_log("DEBUG", &format!("Authenticated incoming connection from {}", remote_id));

    // 2. Read Encrypted Request
    let mut op = [0u8; 1];
    if secure_stream.read_exact(&mut op).await.is_err() {
        return Ok(());
    }

    if op[0] == 0x01 {
        // CONNECT REQUEST
        let target_name = read_length_prefixed_string(&mut secure_stream).await?;

        // 3. Route to Local Service
        if let Some(Target::GapJunction(path)) = routes.get(&target_name) {
            match UnixStream::connect(path).await {
                Ok(target) => {
                    secure_stream.write_all(&[0x00]).await?; // ACK
                    bridge_streams(secure_stream, target).await?;
                }
                Err(_) => {
                    secure_stream.write_all(&[0xFF]).await?; // NACK (Service dead)
                }
            }
        } else {
            // Route not found or we do not allow routing Remote->Remote (prevents amplification attacks)
            sys_log("WARN", &format!("Remote {} requested unknown route: {}", remote_id, target_name));
            secure_stream.write_all(&[0xFF]).await?; // NACK
        }
    }

    Ok(())
}

// --- Helpers ---

async fn accept_tcp_optional(
    listener: Option<&TcpListener>,
) -> Option<std::io::Result<(TcpStream, std::net::SocketAddr)>> {
    match listener {
        Some(l) => Some(l.accept().await),
        None => std::future::pending().await,
    }
}

/// Zero-Copy bidirectional bridge.
/// Works with any combination of UnixStream, TcpStream, or NoiseStream.
async fn bridge_streams<A, B>(a: A, b: B) -> Result<()>
where
    A: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    B: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (mut ra, mut wa) = tokio::io::split(a);
    let (mut rb, mut wb) = tokio::io::split(b);

    // Splice logic happens in kernel where possible
    let _ = tokio::try_join!(
        tokio::io::copy(&mut ra, &mut wb),
        tokio::io::copy(&mut rb, &mut wa)
    );
    Ok(())
}

async fn read_length_prefixed_string<R: tokio::io::AsyncRead + Unpin>(reader: &mut R) -> Result<String> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;

    // Sanity check to prevent OOM attacks
    if len > 1024 * 64 {
        anyhow::bail!("Protocol violation: Target name too long");
    }

    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await?;
    Ok(String::from_utf8(buf).unwrap_or_default())
}

async fn write_length_prefixed_string<W: tokio::io::AsyncWrite + Unpin>(writer: &mut W, s: &str) -> Result<()> {
    let bytes = s.as_bytes();
    writer.write_all(&(bytes.len() as u32).to_be_bytes()).await?;
    writer.write_all(bytes).await?;
    Ok(())
}