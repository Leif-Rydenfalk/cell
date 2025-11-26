//! # Golgi Apparatus (The Router)
//!
//! The Golgi is responsible for packaging data (Vesicles) and routing them to the correct destination.
//! It manages:
//! 1. **Gap Junctions:** Local Unix sockets for high-speed IPC.
//! 2. **Axons:** TCP connections with Noise encryption for remote comms.
//! 3. **Discovery:** Processing Pheromones to update routing tables.
//! 4. **Billing:** Interfacing with Mitochondria to track ATP usage.

pub mod pheromones;

use crate::antigens::Antigens;
use crate::mitochondria::Mitochondria;
use crate::synapse;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UnixListener, UnixStream};
use tokio::sync::RwLock;

/// Represents a remote connection endpoint (Axon).
#[derive(Clone, Debug)]
pub struct AxonTerminal {
    pub id: String,
    pub addr: String,
    pub rtt: Duration,
    pub last_seen: Instant,
    pub is_donor: bool,
}

/// The physical medium used to reach a target cell.
#[derive(Debug)]
pub enum Target {
    /// A direct Unix socket file on the local filesystem.
    /// Provides zero-copy, unencrypted, max-throughput communication.
    GapJunction(PathBuf),

    /// A group of local sockets (Load Balanced).
    LocalColony(Arc<Vec<PathBuf>>),

    /// A remote cluster reachable via TCP/IP.
    /// Requires encryption and serialization overhead.
    AxonCluster(Vec<AxonTerminal>),
}

/// The main Router struct.
pub struct Golgi {
    name: String,
    service_group: String,
    socket_path: PathBuf,
    axon_bind: Option<String>,
    routes: Arc<RwLock<HashMap<String, Target>>>,
    identity: Arc<Antigens>,
    rr_index: AtomicUsize,
    mitochondria: Arc<Mitochondria>,
    is_donor: bool,
}

fn sys_log(level: &str, msg: &str) {
    let timestamp = humantime::format_rfc3339_seconds(SystemTime::now());
    eprintln!("[{}] [{}] [GOLGI] {}", timestamp, level, msg);
}

impl Golgi {
    /// Initialize the Golgi apparatus.
    /// Loads identity and economy (Mitochondria) subsystems.
    pub fn new(
        name: String,
        run_dir: &std::path::Path,
        axon_bind: Option<String>,
        routes_map: HashMap<String, Target>,
        is_donor: bool,
    ) -> Result<Self> {
        let identity_path = run_dir.join("identity");
        let identity =
            Antigens::load_or_create(identity_path).context("Failed to load node identity.")?;

        let mitochondria = Mitochondria::load_or_init(run_dir)?;

        sys_log(
            "INFO",
            &format!(
                "Identity Loaded. ID: {}. ATP Balance: {}",
                identity.public_key_str,
                mitochondria.get_balance()
            ),
        );

        let service_group = name.split('-').next().unwrap_or(&name).to_string();

        Ok(Self {
            name,
            service_group,
            socket_path: run_dir.join("golgi.sock"),
            axon_bind,
            routes: Arc::new(RwLock::new(routes_map)),
            identity: Arc::new(identity),
            rr_index: AtomicUsize::new(0),
            mitochondria: Arc::new(mitochondria),
            is_donor,
        })
    }

    /// Starts the router main loop.
    ///
    /// This spawns:
    /// 1. The Pheromone Discovery task.
    /// 2. The Unix Listener (Local IPC).
    /// 3. The TCP Listener (Remote Access).
    pub async fn run(self) -> Result<()> {
        // Clean up old socket file if it exists (crashed previous run)
        if self.socket_path.exists() {
            tokio::fs::remove_file(&self.socket_path).await.ok();
        }
        let unix_listener = UnixListener::bind(&self.socket_path)
            .context("CRITICAL: Failed to bind internal Gap Junction socket.")?;

        // Bind TCP Axon if configured (for remote access)
        let (tcp_listener, real_port) = if let Some(addr) = &self.axon_bind {
            let l = TcpListener::bind(addr).await.context("Axon bind failed")?;
            let local = l.local_addr()?;
            sys_log("INFO", &format!("Axon Interface online: {}", local));
            (Some(l), local.port())
        } else {
            sys_log(
                "WARN",
                "Node is isolated (No TCP Listener). Client mode only.",
            );
            (None, 0)
        };

        sys_log(
            "INFO",
            &format!("Endocrine System active. Donor Mode: {}", self.is_donor),
        );

        // Convert path to absolute string for broadcasting.
        // This is crucial for neighbors to find the socket file.
        let socket_path_str = self
            .socket_path
            .canonicalize()
            .unwrap_or(self.socket_path.clone())
            .to_string_lossy()
            .to_string();

        let mut rx = pheromones::EndocrineSystem::start(
            self.name.clone(),
            self.service_group.clone(),
            real_port,
            self.identity.public_key_str.clone(),
            self.is_donor,
            Some(socket_path_str), // Advertise local path
        )
        .await?;

        // --- 1. DISCOVERY LOOP ---
        // Listens for Pheromones and updates the routing table.
        let routes_handle = self.routes.clone();
        tokio::spawn(async move {
            while let Some(p) = rx.recv().await {
                let mut table = routes_handle.write().await;
                let target_name = p.service_group.clone();
                let addr_str = p.tcp_addr.to_string();

                // --- OPPORTUNISTIC GAP JUNCTION (Optimization) ---
                // If the peer is local (socket file exists on disk), we bypass TCP/Encryption entirely.
                // This restores ~5GB/s throughput for local communication.
                let mut local_socket_found = false;

                if let Some(path_str) = &p.ipc_socket {
                    let path = PathBuf::from(path_str);
                    if path.exists() {
                        // FOUND LOCAL SOCKET: Upgrade the route!

                        // We only log this once per discovery to avoid console spam
                        if !matches!(table.get(&target_name), Some(Target::GapJunction(_))) {
                            sys_log(
                                "INFO",
                                &format!(
                                    "Upgrading route '{}' to Gap Junction (Local Optimization)",
                                    p.cell_name
                                ),
                            );
                        }

                        // Overwrite any existing TCP route with the fast local path
                        table.insert(target_name.clone(), Target::GapJunction(path));
                        local_socket_found = true;
                    }
                }

                // If no local socket found, fallback to standard TCP (Axon) logic
                if !local_socket_found {
                    let entry = table
                        .entry(target_name.clone())
                        .or_insert(Target::AxonCluster(Vec::new()));

                    if let Target::AxonCluster(cluster) = entry {
                        if let Some(existing) = cluster.iter_mut().find(|t| t.id == p.cell_name) {
                            existing.last_seen = Instant::now();
                            existing.addr = addr_str;
                        } else {
                            sys_log(
                                "INFO",
                                &format!(
                                    "Discovered: {} via TCP (Donor: {})",
                                    p.cell_name, p.is_donor
                                ),
                            );
                            cluster.push(AxonTerminal {
                                id: p.cell_name,
                                addr: addr_str,
                                rtt: Duration::from_millis(999),
                                last_seen: Instant::now(),
                                is_donor: p.is_donor,
                            });
                        }
                    }
                }
            }
        });

        // --- 2. TRANSPORT LOOP ---
        // Handles incoming connections from local processes or remote TCP clients.
        let rr_index = Arc::new(self.rr_index);
        let routes = self.routes.clone();
        let identity = self.identity.clone();
        let mitochondria = self.mitochondria.clone();

        loop {
            tokio::select! {
                // A. Local Request (via Gap Junction)
                res = unix_listener.accept() => {
                    if let Ok((stream, _)) = res {
                        let r = routes.clone();
                        let i = identity.clone();
                        let rr = rr_index.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_local_signal(stream, r, i, rr).await {
                                sys_log("ERROR", &format!("Local: {}", e));
                            }
                        });
                    }
                }
                // B. Remote Request (via Axon/TCP)
                res = accept_tcp_optional(tcp_listener.as_ref()) => {
                    if let Some(Ok((stream, addr))) = res {
                        let r = routes.clone();
                        let i = identity.clone();
                        let rr = rr_index.clone();
                        let m = mitochondria.clone();
                        let is_donor_node = self.is_donor;
                        tokio::spawn(async move {
                            if let Err(e) = handle_remote_signal(stream, addr, r, i, rr, m, is_donor_node).await {
                                // Filter out common connection reset noises
                                let msg = e.to_string();
                                if !msg.contains("early eof") && !msg.contains("Probe dropped") {
                                    sys_log("WARN", &format!("Remote {}: {}", addr, msg));
                                }
                            }
                        });
                    }
                }
            }
        }
    }
}

/// Helper to round-robin connect to a cluster of local worker sockets.
async fn connect_to_colony_with_retry(
    sockets: &Arc<Vec<PathBuf>>,
    rr: &Arc<AtomicUsize>,
) -> Option<UnixStream> {
    if sockets.is_empty() {
        return None;
    }
    let attempts = std::cmp::min(3, sockets.len());
    for _ in 0..attempts {
        let idx = rr.fetch_add(1, Ordering::Relaxed) % sockets.len();
        match UnixStream::connect(&sockets[idx]).await {
            Ok(s) => return Some(s),
            Err(_) => continue,
        }
    }
    None
}

/// Handles a request coming from the local machine (SDK -> Golgi).
/// Determines the destination and bridges the connection.
async fn handle_local_signal(
    mut stream: UnixStream,
    routes: Arc<RwLock<HashMap<String, Target>>>,
    identity: Arc<Antigens>,
    rr_index: Arc<AtomicUsize>,
) -> Result<()> {
    // 1. Read Opcode
    let mut op = [0u8; 1];
    if stream.read_exact(&mut op).await.is_err() {
        return Ok(());
    }

    // Op 0x01: Connect/RPC
    if op[0] == 0x01 {
        // 2. Read Target Service Name
        let target_name = read_len_str(&mut stream).await?;

        // 3. Routing Logic
        let chosen_route = {
            let r = routes.read().await;
            match r.get(&target_name) {
                Some(Target::LocalColony(sockets)) => Some(RouteChoice::Colony(sockets.clone())),
                Some(Target::GapJunction(path)) => Some(RouteChoice::Unix(path.clone())),
                Some(Target::AxonCluster(cluster)) => {
                    // Simple Load Balancing for remote cluster
                    if cluster.is_empty() {
                        None
                    } else {
                        let idx = rr_index.fetch_add(1, Ordering::Relaxed) % cluster.len();
                        Some(RouteChoice::Tcp(cluster[idx].addr.clone()))
                    }
                }
                None => None,
            }
        };

        // 4. Connection Bridging
        match chosen_route {
            // Case A: Local Load Balanced Colony
            Some(RouteChoice::Colony(sockets)) => {
                match connect_to_colony_with_retry(&sockets, &rr_index).await {
                    Some(target) => {
                        stream.write_all(&[0x00]).await?; // ACK
                        bridge_plain(stream, target).await?;
                    }
                    None => stream.write_all(&[0xFF]).await?, // NACK
                }
            }
            // Case B: Direct Unix Socket (Fast Path / Gap Junction)
            // This is where the optimization shines. No encryption, pure OS pipe.
            Some(RouteChoice::Unix(path)) => {
                match UnixStream::connect(path).await {
                    Ok(target) => {
                        stream.write_all(&[0x00]).await?; // ACK
                        bridge_plain(stream, target).await?;
                    }
                    Err(_) => stream.write_all(&[0xFF]).await?, // NACK
                }
            }
            // Case C: Remote TCP (Slow Path / Axon)
            // Requires Noise handshake and framing.
            Some(RouteChoice::Tcp(addr)) => {
                let tcp_stream = TcpStream::connect(addr).await?;
                tcp_stream.set_nodelay(true)?;

                // Handshake with Remote Golgi
                let (mut secure_stream, _) =
                    synapse::connect_secure(tcp_stream, &identity.keypair, true).await?;

                {
                    // Send Connect Frame
                    let mut buf = vec![0u8; 512];
                    let mut payload = vec![0x01];
                    payload.extend(&(target_name.len() as u32).to_be_bytes());
                    payload.extend(target_name.as_bytes());
                    let len = secure_stream
                        .state
                        .write_message(&payload, &mut buf)
                        .unwrap();
                    synapse::write_frame(&mut secure_stream.inner, &buf[..len]).await?;
                }

                // Wait for Remote ACK
                let frame = synapse::read_frame(&mut secure_stream.inner).await?;
                let mut buf = vec![0u8; 1024];
                let len = secure_stream.state.read_message(&frame, &mut buf)?;

                if len > 0 && buf[0] == 0x00 {
                    stream.write_all(&[0x00]).await?;
                    synapse::bridge_secure_to_plain(secure_stream, stream).await?;
                } else {
                    stream.write_all(&[0xFF]).await?;
                }
            }
            _ => stream.write_all(&[0xFF]).await?,
        }
    }
    Ok(())
}

/// Handles a request coming from the network (Axon -> Golgi).
/// Always requires Noise decryption and potentially billing.
async fn handle_remote_signal(
    stream: TcpStream,
    _addr: std::net::SocketAddr,
    routes: Arc<RwLock<HashMap<String, Target>>>,
    identity: Arc<Antigens>,
    rr_index: Arc<AtomicUsize>,
    mitochondria: Arc<Mitochondria>,
    is_donor: bool,
) -> Result<()> {
    stream.set_nodelay(true)?;

    // 1. SECURE HANDSHAKE (Mutual Auth)
    let (mut secure_stream, remote_pub) =
        synapse::connect_secure(stream, &identity.keypair, false).await?;

    // Identity is the public key (base64)
    let remote_id = base64::encode(remote_pub);

    let frame = synapse::read_frame(&mut secure_stream.inner).await?;
    let mut buf = vec![0u8; 1024];
    let len = secure_stream.state.read_message(&frame, &mut buf)?;

    if len < 5 {
        return Ok(());
    }

    // 2. ROUTING
    if buf[0] == 0x01 {
        let name_len = u32::from_be_bytes(buf[1..5].try_into()?) as usize;
        let target_name = String::from_utf8(buf[5..5 + name_len].to_vec())?;

        let start_time = Instant::now();

        // Remote requests can only route to local resources.
        // We generally do not relay TCP-to-TCP to prevent being used as a proxy.
        let chosen_route = {
            let r = routes.read().await;
            match r.get(&target_name) {
                Some(Target::LocalColony(sockets)) => Some(RouteChoice::Colony(sockets.clone())),
                Some(Target::GapJunction(path)) => Some(RouteChoice::Unix(path.clone())),
                _ => None,
            }
        };

        match chosen_route {
            Some(RouteChoice::Colony(sockets)) => {
                match connect_to_colony_with_retry(&sockets, &rr_index).await {
                    Some(target) => {
                        // ACK
                        let len = secure_stream
                            .state
                            .write_message(&[0x00], &mut buf)
                            .unwrap();
                        synapse::write_frame(&mut secure_stream.inner, &buf[..len]).await?;

                        // BRIDGE: Network -> Decrypt -> Unix
                        synapse::bridge_secure_to_plain(secure_stream, target).await?;

                        // BILLING
                        let duration_ms = start_time.elapsed().as_millis() as u64;
                        if is_donor {
                            let _ =
                                mitochondria.synthesize_atp(&remote_id, &target_name, duration_ms);
                        }
                    }
                    None => {
                        // NACK
                        let len = secure_stream
                            .state
                            .write_message(&[0xFF], &mut buf)
                            .unwrap();
                        synapse::write_frame(&mut secure_stream.inner, &buf[..len]).await?;
                    }
                }
            }
            Some(RouteChoice::Unix(path)) => {
                match UnixStream::connect(path).await {
                    Ok(target) => {
                        // ACK
                        let len = secure_stream
                            .state
                            .write_message(&[0x00], &mut buf)
                            .unwrap();
                        synapse::write_frame(&mut secure_stream.inner, &buf[..len]).await?;

                        synapse::bridge_secure_to_plain(secure_stream, target).await?;

                        let duration_ms = start_time.elapsed().as_millis() as u64;
                        if is_donor {
                            let _ =
                                mitochondria.synthesize_atp(&remote_id, &target_name, duration_ms);
                        }
                    }
                    Err(_) => {
                        // NACK
                        let len = secure_stream
                            .state
                            .write_message(&[0xFF], &mut buf)
                            .unwrap();
                        synapse::write_frame(&mut secure_stream.inner, &buf[..len]).await?;
                    }
                }
            }
            _ => {
                // Route not found / Deny
                let len = secure_stream
                    .state
                    .write_message(&[0xFF], &mut buf)
                    .unwrap();
                synapse::write_frame(&mut secure_stream.inner, &buf[..len]).await?;
            }
        }
    }
    Ok(())
}

// --- Helper Types & Functions ---

enum RouteChoice {
    Unix(PathBuf),
    Colony(Arc<Vec<PathBuf>>),
    Tcp(String),
}

async fn accept_tcp_optional(
    l: Option<&TcpListener>,
) -> Option<std::io::Result<(TcpStream, std::net::SocketAddr)>> {
    match l {
        Some(l) => Some(l.accept().await),
        None => std::future::pending().await,
    }
}

async fn read_len_str<R: tokio::io::AsyncRead + Unpin>(r: &mut R) -> Result<String> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    Ok(String::from_utf8(buf)?)
}

/// Zero-copy bridge between two Unix streams.
/// Used for Gap Junctions (Local -> Local).
async fn bridge_plain<A, B>(a: A, b: B) -> Result<()>
where
    A: AsyncReadExt + AsyncWriteExt + Unpin + Send + 'static,
    B: AsyncReadExt + AsyncWriteExt + Unpin + Send + 'static,
{
    let (mut ra, mut wa) = tokio::io::split(a);
    let (mut rb, mut wb) = tokio::io::split(b);
    let _ = tokio::try_join!(
        tokio::io::copy(&mut ra, &mut wb),
        tokio::io::copy(&mut rb, &mut wa)
    );
    Ok(())
}
