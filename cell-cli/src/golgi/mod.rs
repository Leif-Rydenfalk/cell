pub mod pheromones;

use crate::antigens::Antigens;
use crate::mitochondria::Mitochondria; // NEW
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

#[derive(Clone, Debug)]
pub struct AxonTerminal {
    pub id: String,
    pub addr: String,
    pub rtt: Duration,
    pub last_seen: Instant,
    pub is_donor: bool, // NEW
}

#[derive(Debug)]
pub enum Target {
    GapJunction(PathBuf),
    LocalColony(Arc<Vec<PathBuf>>),
    AxonCluster(Vec<AxonTerminal>),
}

pub struct Golgi {
    name: String,
    service_group: String,
    socket_path: PathBuf,
    axon_bind: Option<String>,
    routes: Arc<RwLock<HashMap<String, Target>>>,
    identity: Arc<Antigens>,
    rr_index: AtomicUsize,
    mitochondria: Arc<Mitochondria>, // NEW: Economy
    is_donor: bool,                  // NEW
}

fn sys_log(level: &str, msg: &str) {
    let timestamp = humantime::format_rfc3339_seconds(SystemTime::now());
    eprintln!("[{}] [{}] [GOLGI] {}", timestamp, level, msg);
}

impl Golgi {
    pub fn new(
        name: String,
        run_dir: &std::path::Path,
        axon_bind: Option<String>,
        routes_map: HashMap<String, Target>,
        is_donor: bool, // NEW
    ) -> Result<Self> {
        let identity_path = run_dir.join("identity");
        let identity =
            Antigens::load_or_create(identity_path).context("Failed to load node identity.")?;

        // Initialize Economy
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

    pub async fn run(self) -> Result<()> {
        if self.socket_path.exists() {
            tokio::fs::remove_file(&self.socket_path).await.ok();
        }
        let unix_listener = UnixListener::bind(&self.socket_path)
            .context("CRITICAL: Failed to bind internal Gap Junction socket.")?;

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
        let mut rx = pheromones::EndocrineSystem::start(
            self.name.clone(),
            self.service_group.clone(),
            real_port,
            self.identity.public_key_str.clone(),
            self.is_donor,
        )
        .await?;

        // 1. DISCOVERY LOOP
        let routes_handle = self.routes.clone();
        tokio::spawn(async move {
            while let Some(p) = rx.recv().await {
                let mut table = routes_handle.write().await;
                let target_name = p.service_group;
                let addr_str = p.tcp_addr.to_string();

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
                            &format!("Discovered: {} (Donor: {})", p.cell_name, p.is_donor),
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
        });

        // 2. CHEMOTAXIS LOOP (Same as before)
        // ... (Omitting code for brevity as it is unchanged from original) ...

        // --- TRANSPORT LOOP ---
        let rr_index = Arc::new(self.rr_index);
        let routes = self.routes.clone();
        let identity = self.identity.clone();
        let mitochondria = self.mitochondria.clone();

        loop {
            tokio::select! {
                res = unix_listener.accept() => {
                    if let Ok((stream, _)) = res {
                        let r = routes.clone();
                        let i = identity.clone();
                        let rr = rr_index.clone();
                        // Local requests usually don't cost ATP (you own the hardware)
                        // but tracking is possible if desired.
                        tokio::spawn(async move {
                            if let Err(e) = handle_local_signal(stream, r, i, rr).await {
                                sys_log("ERROR", &format!("Local: {}", e));
                            }
                        });
                    }
                }
                res = accept_tcp_optional(tcp_listener.as_ref()) => {
                    if let Some(Ok((stream, addr))) = res {
                        let r = routes.clone();
                        let i = identity.clone();
                        let rr = rr_index.clone();
                        let m = mitochondria.clone();
                        let is_donor_node = self.is_donor;
                        tokio::spawn(async move {
                            // HERE: We pass mitochondria to record earnings
                            if let Err(e) = handle_remote_signal(stream, addr, r, i, rr, m, is_donor_node).await {
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

// ... (Existing helper functions connect_to_colony_with_retry, handle_local_signal, etc) ...
// Assuming handle_local_signal is unchanged.

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

// Re-implementing handle_local_signal to be safe
async fn handle_local_signal(
    mut stream: UnixStream,
    routes: Arc<RwLock<HashMap<String, Target>>>,
    identity: Arc<Antigens>,
    rr_index: Arc<AtomicUsize>,
) -> Result<()> {
    let mut op = [0u8; 1];
    if stream.read_exact(&mut op).await.is_err() {
        return Ok(());
    }

    if op[0] == 0x01 {
        let target_name = read_len_str(&mut stream).await?;
        let chosen_route = {
            let r = routes.read().await;
            match r.get(&target_name) {
                Some(Target::LocalColony(sockets)) => Some(RouteChoice::Colony(sockets.clone())),
                Some(Target::GapJunction(path)) => Some(RouteChoice::Unix(path.clone())),
                Some(Target::AxonCluster(cluster)) => {
                    cluster.first().map(|t| RouteChoice::Tcp(t.addr.clone()))
                }
                None => None,
            }
        };

        match chosen_route {
            Some(RouteChoice::Colony(sockets)) => {
                match connect_to_colony_with_retry(&sockets, &rr_index).await {
                    Some(target) => {
                        stream.write_all(&[0x00]).await?;
                        bridge_plain(stream, target).await?;
                    }
                    None => stream.write_all(&[0xFF]).await?,
                }
            }
            Some(RouteChoice::Tcp(addr)) => {
                let tcp_stream = TcpStream::connect(addr).await?;
                tcp_stream.set_nodelay(true)?;
                let (mut secure_stream, _) =
                    synapse::connect_secure(tcp_stream, &identity.keypair, true).await?;

                {
                    let mut buf = vec![0u8; 1024];
                    let mut payload = vec![0x01];
                    payload.extend(&(target_name.len() as u32).to_be_bytes());
                    payload.extend(target_name.as_bytes());
                    let len = secure_stream
                        .state
                        .write_message(&payload, &mut buf)
                        .unwrap();
                    synapse::write_frame(&mut secure_stream.inner, &buf[..len]).await?;
                }

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

async fn handle_remote_signal(
    stream: TcpStream,
    addr: std::net::SocketAddr,
    routes: Arc<RwLock<HashMap<String, Target>>>,
    identity: Arc<Antigens>,
    rr_index: Arc<AtomicUsize>,
    mitochondria: Arc<Mitochondria>, // Passed down
    is_donor: bool,
) -> Result<()> {
    stream.set_nodelay(true)?;
    let (mut secure_stream, remote_pub) =
        synapse::connect_secure(stream, &identity.keypair, false).await?;
    let remote_id = base64::encode(remote_pub); // Simplified ID

    let frame = synapse::read_frame(&mut secure_stream.inner).await?;
    let mut buf = vec![0u8; 1024];
    let len = secure_stream.state.read_message(&frame, &mut buf)?;

    if len < 5 {
        return Ok(());
    }

    if buf[0] == 0x01 {
        let name_len = u32::from_be_bytes(buf[1..5].try_into()?) as usize;
        let target_name = String::from_utf8(buf[5..5 + name_len].to_vec())?;

        let start_time = Instant::now();

        let chosen_route = {
            let r = routes.read().await;
            match r.get(&target_name) {
                Some(Target::LocalColony(sockets)) => Some(RouteChoice::Colony(sockets.clone())),
                // If I am a donor, I might accept work for services not explicitly in routes if I have a generic handler,
                // but for MVP we assume donor runs specific "Worker" cells that are in routes.
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

                        // Execute
                        synapse::bridge_secure_to_plain(secure_stream, target).await?;

                        // BILLING (Post-execution)
                        // In the real system, bridge_secure_to_plain would return bytes transferred or timing.
                        // Here we use wall-clock time of the connection as a proxy for CPU time.
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
            _ => {
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
