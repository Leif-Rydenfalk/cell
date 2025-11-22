use anyhow::{anyhow, Context, Result};
use cell_cli::genesis::run_genesis;
use clap::{Parser, Subcommand};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::{TcpStream, UdpSocket}; // Added UdpSocket
use tokio::sync::Mutex;

// Import from internal lib
use cell_cli::golgi::{pheromones, Golgi, Target}; // Import pheromones
use cell_cli::{antigens, nucleus, synapse, sys_log, vacuole};

#[derive(Parser)]
#[command(name = "membrane")]
struct Cli {
    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    Mitosis { dir: PathBuf },
}

#[derive(Deserialize, Debug, Clone)]
struct Genome {
    genome: Option<CellTraits>,
    #[serde(default)]
    axons: HashMap<String, String>,
    #[serde(default)]
    junctions: HashMap<String, String>,
    workspace: Option<WorkspaceTraits>,
}

#[derive(Deserialize, Debug, Clone)]
struct WorkspaceTraits {
    members: Vec<String>,
}

#[derive(Deserialize, Debug, Clone)]
struct CellTraits {
    name: String,
    #[serde(default)]
    listen: Option<String>,
    #[serde(default)]
    replicas: Option<u32>,
}

type CellRegistry = HashMap<String, PathBuf>;
type RunningSet = Arc<Mutex<HashSet<String>>>;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.action {
        Action::Mitosis { dir } => mitosis(&dir).await,
    }
}

async fn mitosis(dir: &Path) -> Result<()> {
    let dir = dir.canonicalize().context("Invalid directory")?;
    let genome_path = dir.join("genome.toml");

    sys_log("INFO", "Scanning workspace for cellular life...");
    let mut registry = CellRegistry::new();
    inventory_cells(&dir, &mut registry)?;
    sys_log(
        "INFO",
        &format!("Discovered {} local cells.", registry.len()),
    );

    let txt = std::fs::read_to_string(&genome_path).context("Missing genome.toml")?;
    let dna: Genome = toml::from_str(&txt)?;
    let running = Arc::new(Mutex::new(HashSet::new()));

    if let Some(ws) = dna.workspace {
        sys_log("INFO", "Workspace detected. Resolving dependency graph...");
        for member_dir in ws.members {
            let path = dir.join(member_dir);
            let m_txt = std::fs::read_to_string(path.join("genome.toml"))?;
            let m_dna: Genome = toml::from_str(&m_txt)?;
            if let Some(traits) = m_dna.genome {
                ensure_active(&traits.name, &registry, running.clone(), false).await?;
            }
        }
        sys_log(
            "INFO",
            "Cluster fully operational. Press Ctrl+C to shutdown.",
        );
        tokio::signal::ctrl_c().await?;
    } else if let Some(traits) = dna.genome {
        ensure_active(&traits.name, &registry, running.clone(), true).await?;
    }

    Ok(())
}

async fn ensure_active(
    cell_name: &str,
    registry: &CellRegistry,
    running: RunningSet,
    is_root: bool,
) -> Result<()> {
    {
        let mut set = running.lock().await;
        if set.contains(cell_name) {
            return Ok(());
        }
        set.insert(cell_name.to_string());
    }

    let cell_path = registry.get(cell_name).ok_or_else(|| {
        anyhow!(
            "Cell '{}' not found in local workspace. Cannot auto-boot.",
            cell_name
        )
    })?;

    let txt = std::fs::read_to_string(cell_path.join("genome.toml"))?;
    let dna: Genome = toml::from_str(&txt)?;

    sys_log("INFO", &format!("[{}] Checking dependencies...", cell_name));

    for (axon_name, axon_addr) in &dna.axons {
        // 1. Resolve Address (DNS, IP, or Pheromone Discovery)
        let target_addr = resolve_address(axon_name, axon_addr).await;

        match target_addr {
            Ok(addr) => {
                // 2. Verify Connectivity
                if verify_tcp(&addr).await.is_ok() {
                    sys_log(
                        "INFO",
                        &format!(
                            "[{}] Dependency '{}' found at {}",
                            cell_name, axon_name, addr
                        ),
                    );
                    continue; // Found it!
                }
            }
            Err(_) => { /* Resolution failed, try booting local */ }
        }

        // 3. If Resolution or Connection failed, try booting from source
        if registry.contains_key(axon_name) {
            sys_log(
                "WARN",
                &format!(
                    "[{}] Dependency '{}' not reachable. Booting local instance...",
                    cell_name, axon_name
                ),
            );
            Box::pin(ensure_active(axon_name, registry, running.clone(), false)).await?;

            // Wait for it to come up
            let mut attempts = 0;
            loop {
                // Re-resolve locally
                if let Ok(addr) = resolve_address(axon_name, axon_addr).await {
                    if verify_tcp(&addr).await.is_ok() {
                        break;
                    }
                }
                attempts += 1;
                if attempts > 10 {
                    anyhow::bail!("Dependency '{}' failed to boot.", axon_name);
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        } else {
            anyhow::bail!("CRITICAL: Dependency '{}' is missing from network AND local workspace. Cannot compile.", axon_name);
        }
    }

    sys_log(
        "INFO",
        &format!("[{}] Dependencies verified. Running Genesis...", cell_name),
    );
    run_genesis(cell_path)?;

    // This will now use Pheromone discovery if needed
    snapshot_genomes(cell_path, &dna.axons).await?;

    sys_log("INFO", &format!("[{}] Compiling...", cell_name));
    let bin_path = compile_cell(cell_path, cell_name)?;

    if is_root {
        sys_log("INFO", &format!("[{}] Starting (Foreground)...", cell_name));
        run_cell_runtime(cell_path, &dna, bin_path).await?;
    } else {
        sys_log("INFO", &format!("[{}] Spawning (Background)...", cell_name));
        spawn_cell_background(cell_path, &bin_path)?;
    }

    Ok(())
}

// --- NETWORK RESOLVER ---

async fn resolve_address(name: &str, raw_addr: &str) -> Result<String> {
    let clean = raw_addr.replace("axon://", "");

    // 1. Try parsing as direct IP/DNS
    if clean.contains(':') {
        if tokio::net::lookup_host(&clean).await.is_ok() {
            return Ok(clean);
        }
    }

    // 2. Pheromone Discovery (UDP Multicast)
    sys_log(
        "INFO",
        &format!("Searching network for '{}' (Pheromones)...", name),
    );

    // Create a standard UDP socket for listening (reuse port logic similar to pheromones.rs)
    let socket = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let fd = socket.as_raw_fd();
        unsafe {
            let opt = 1;
            libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_REUSEPORT,
                &opt as *const _ as *const libc::c_void,
                4,
            );
        }
    }
    socket.bind(&"0.0.0.0:9099".parse::<SocketAddr>()?.into())?;
    socket.join_multicast_v4(&"239.255.0.1".parse()?, &std::net::Ipv4Addr::UNSPECIFIED)?;
    socket.set_nonblocking(true)?;

    let udp = UdpSocket::from_std(socket.into())?;
    let mut buf = [0u8; 2048];
    let start = Instant::now();

    // Listen for 3 seconds
    while start.elapsed() < Duration::from_secs(3) {
        if let Ok(len) = udp.try_recv(&mut buf) {
            if let Ok(p) = serde_json::from_slice::<pheromones::Pheromone>(&buf[..len]) {
                if p.cell_name == name || p.service_group == name {
                    return Ok(p.tcp_addr.to_string());
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    anyhow::bail!("Resolution failed for {}", name)
}

async fn verify_tcp(addr: &str) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(1), TcpStream::connect(addr)).await??;
    Ok(())
}

// --- RUNTIME HELPERS ---

fn compile_cell(dir: &Path, name: &str) -> Result<PathBuf> {
    let output = Command::new("cargo")
        .args(&["build", "--release", "--message-format=json"])
        .current_dir(dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output()?;

    if !output.status.success() {
        anyhow::bail!("Compilation failed for {}", name);
    }

    let reader = std::io::BufReader::new(output.stdout.as_slice());
    use std::io::BufRead;
    for line in reader.lines() {
        if let Ok(l) = line {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&l) {
                if val["reason"] == "compiler-artifact" && val["target"]["name"] == name {
                    if let Some(executable) = val["executable"].as_str() {
                        return Ok(PathBuf::from(executable));
                    }
                }
            }
        }
    }
    anyhow::bail!("Binary not found for {}", name);
}

fn spawn_cell_background(dir: &Path, _bin: &Path) -> Result<()> {
    let self_exe = std::env::current_exe()?;
    let log_file = std::fs::File::create(dir.join("run/daemon.log"))?;

    Command::new(self_exe)
        .arg("mitosis")
        .arg(dir)
        .stdout(log_file.try_clone()?)
        .stderr(log_file)
        .spawn()
        .context("Failed to spawn background cell")?;
    Ok(())
}

async fn run_cell_runtime(dir: &Path, dna: &Genome, bin_path: PathBuf) -> Result<()> {
    let run_dir = dir.join("run");
    if run_dir.exists() {
        std::fs::remove_dir_all(&run_dir)?;
    }
    std::fs::create_dir_all(&run_dir)?;

    let traits = dna.genome.as_ref().unwrap();
    let mut routes = HashMap::new();
    let golgi_sock_path = run_dir.join("golgi.sock");

    let cell_sock = run_dir.join("cell.sock");
    let log_path = run_dir.join("service.log");

    let guard = nucleus::activate(
        &cell_sock,
        nucleus::LogStrategy::File(log_path),
        &bin_path,
        &golgi_sock_path,
    )?;

    routes.insert(traits.name.clone(), Target::GapJunction(cell_sock));
    for (name, path) in &dna.junctions {
        routes.insert(
            name.clone(),
            Target::GapJunction(dir.join(path).join("run/cell.sock")),
        );
    }

    // IMPORTANT: Pass the RAW axon addresses to Golgi.
    // Golgi has its own continuous discovery loop.
    for (name, addr) in &dna.axons {
        let clean = addr.replace("axon://", "");
        routes.insert(
            name.clone(),
            Target::AxonCluster(vec![cell_cli::golgi::AxonTerminal {
                id: "static".into(),
                addr: clean,
                rtt: Duration::from_secs(1),
                last_seen: Instant::now(),
            }]),
        );
    }

    let golgi = Golgi::new(traits.name.clone(), &run_dir, traits.listen.clone(), routes)?;

    // Keep guard alive
    let _g = guard;

    tokio::select! {
        _ = golgi.run() => {},
        _ = tokio::signal::ctrl_c() => {}
    }
    Ok(())
}

fn inventory_cells(dir: &Path, registry: &mut CellRegistry) -> Result<()> {
    if dir.is_dir() {
        let genome_file = dir.join("genome.toml");
        if genome_file.exists() {
            if let Ok(txt) = std::fs::read_to_string(&genome_file) {
                if let Ok(g) = toml::from_str::<Genome>(&txt) {
                    if let Some(t) = g.genome {
                        registry.insert(t.name, dir.to_path_buf());
                    }
                    if let Some(ws) = g.workspace {
                        for m in ws.members {
                            inventory_cells(&dir.join(m), registry)?;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

async fn snapshot_genomes(root: &Path, axons: &HashMap<String, String>) -> Result<()> {
    let schema_dir = root.join(".cell-genomes");
    std::fs::create_dir_all(&schema_dir)?;
    let temp_id_path = root.join("run/temp_builder_identity");
    let identity = antigens::Antigens::load_or_create(temp_id_path)?;

    for (name, raw_addr) in axons {
        // 1. Resolve
        let addr = resolve_address(name, raw_addr).await?;

        sys_log(
            "INFO",
            &format!("Snapshotting schema from {} ({})", name, addr),
        );

        let mut connected = false;
        // 2. Connect & Download
        if let Ok(stream) = TcpStream::connect(&addr).await {
            if let Ok((mut secure, _)) =
                synapse::connect_secure(stream, &identity.keypair, true).await
            {
                let mut buf = vec![0u8; 4096];

                // Handshake
                let mut payload = vec![0x01];
                payload.extend(&(name.len() as u32).to_be_bytes());
                payload.extend(name.as_bytes());
                let len = secure.state.write_message(&payload, &mut buf).unwrap();
                synapse::write_frame(&mut secure.inner, &buf[..len]).await?;

                let frame = synapse::read_frame(&mut secure.inner).await?;
                let len = secure.state.read_message(&frame, &mut buf)?;
                if len > 0 && buf[0] == 0x00 {
                    // Request
                    let req = b"__GENOME__";
                    let mut v = (req.len() as u32).to_be_bytes().to_vec();
                    v.extend_from_slice(req);
                    let len = secure.state.write_message(&v, &mut buf).unwrap();
                    synapse::write_frame(&mut secure.inner, &buf[..len]).await?;

                    // Response
                    let frame = synapse::read_frame(&mut secure.inner).await?;
                    let len = secure.state.read_message(&frame, &mut buf)?;
                    if len >= 4 {
                        let jlen = u32::from_be_bytes(buf[0..4].try_into().unwrap()) as usize;
                        if len >= 4 + jlen {
                            let json = &buf[4..4 + jlen];
                            std::fs::write(schema_dir.join(format!("{}.json", name)), json)?;
                            connected = true;
                        }
                    }
                }
            }
        }

        if !connected {
            anyhow::bail!(
                "CRITICAL: Could not download schema for '{}' from {}. Compilation aborted.",
                name,
                addr
            );
        }
    }
    Ok(())
}
