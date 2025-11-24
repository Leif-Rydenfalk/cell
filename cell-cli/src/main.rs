use anyhow::{anyhow, Context, Result};
use cell_cli::genesis::run_genesis;
use clap::{Parser, Subcommand};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::{broadcast, Mutex};
use tokio::task::JoinSet;

// Import from internal lib
use cell_cli::golgi::{pheromones, Golgi, Target};
use cell_cli::{antigens, mitochondria, nucleus, synapse, sys_log, vacuole};

#[derive(Parser)]
#[command(name = "membrane")]
struct Cli {
    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Boots the Cell Runtime.
    Mitosis {
        dir: PathBuf,
        /// Enable Donor Mode: Offer compute resources to the network in exchange for ATP.
        #[arg(long)]
        donor: bool,
    },
    /// Manage financial resources (ATP).
    Wallet { dir: PathBuf },
}

#[derive(Deserialize, Debug, Clone)]
struct Genome {
    genome: Option<CellTraits>,
    #[serde(default)]
    axons: HashMap<String, String>,
    #[serde(default)]
    junctions: HashMap<String, String>,
    #[serde(default)]
    sources: HashMap<String, String>,
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
        Action::Mitosis { dir, donor } => mitosis(&dir, donor).await,
        Action::Wallet { dir } => wallet(&dir).await,
    }
}

async fn wallet(dir: &Path) -> Result<()> {
    let run_dir = dir.join("run");
    if !run_dir.exists() {
        // Try looking in current dir if not found (development convenience)
        if !dir.join("mitochondria.json").exists() {
            anyhow::bail!(
                "No runtime data found at {}. Have you run 'mitosis' yet?",
                dir.display()
            );
        }
    }

    // In a real run, mitochondria.json might be in root or run/, checking both for flexibility
    let ledger_root = if dir.join("mitochondria.json").exists() {
        dir
    } else {
        &run_dir
    };

    let mito = mitochondria::Mitochondria::load_or_init(ledger_root)?;
    mito.print_statement();
    Ok(())
}

async fn mitosis(dir: &Path, is_donor: bool) -> Result<()> {
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
                ensure_active(&traits.name, &registry, running.clone(), false, is_donor).await?;
            }
        }
        sys_log(
            "INFO",
            "Cluster fully operational. Press Ctrl+C to shutdown.",
        );
        tokio::signal::ctrl_c().await?;
    } else if let Some(traits) = dna.genome {
        ensure_active(&traits.name, &registry, running.clone(), true, is_donor).await?;
    }

    Ok(())
}

async fn ensure_active(
    cell_name: &str,
    registry: &CellRegistry,
    running: RunningSet,
    is_root: bool,
    is_donor: bool,
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
            // Recursively boot dependency
            // Note: Dependencies spawned this way inherit the 'donor' status
            Box::pin(ensure_active(
                axon_name,
                registry,
                running.clone(),
                false,
                is_donor,
            ))
            .await?;

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
            // In the future, this is where we would check the content-addressed network
            // to download the binary if we have enough ATP.
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
        run_cell_runtime(cell_path, &dna, bin_path, is_donor).await?;
    } else {
        sys_log("INFO", &format!("[{}] Spawning (Background)...", cell_name));
        spawn_cell_background(cell_path, &bin_path, is_donor)?;
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

    // Create a standard UDP socket for listening
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

fn spawn_cell_background(dir: &Path, _bin: &Path, is_donor: bool) -> Result<()> {
    let self_exe = std::env::current_exe()?;
    let log_file = std::fs::File::create(dir.join("run/daemon.log"))?;

    let mut cmd = Command::new(self_exe);
    cmd.arg("mitosis").arg(dir);

    if is_donor {
        cmd.arg("--donor");
    }

    cmd.stdout(log_file.try_clone()?)
        .stderr(log_file)
        .spawn()
        .context("Failed to spawn background cell")?;
    Ok(())
}

// The Runtime Loop
async fn run_cell_runtime(
    dir: &Path,
    dna: &Genome,
    bin_path: PathBuf,
    is_donor: bool,
) -> Result<()> {
    let run_dir = dir.join("run");
    if run_dir.exists() {
        std::fs::remove_dir_all(&run_dir)?;
    }
    std::fs::create_dir_all(&run_dir)?;

    let traits = dna.genome.as_ref().unwrap();
    let mut routes = HashMap::new();
    let golgi_sock_path = run_dir.join("golgi.sock");

    // 1. Create Shutdown Channel
    let (shutdown_tx, _) = broadcast::channel::<()>(1);

    // 2. Create Task Tracker
    let mut monitors = JoinSet::new();

    let replicas = traits.replicas.unwrap_or(1);

    if replicas > 1 {
        sys_log("INFO", &format!("Spawning Colony: {} workers.", replicas));
        let socket_dir = run_dir.join("sockets");
        std::fs::create_dir_all(&socket_dir)?;

        let log_path = run_dir.join("service.log");
        let vacuole = Arc::new(vacuole::Vacuole::new(log_path).await?);
        let mut worker_sockets = Vec::new();

        for i in 0..replicas {
            let worker_dir = run_dir.join("workers").join(i.to_string());
            std::fs::create_dir_all(&worker_dir)?;
            let sock_path = worker_dir.join("cell.sock");
            worker_sockets.push(sock_path.clone());

            let mut guard = nucleus::activate(
                &sock_path,
                nucleus::LogStrategy::Piped,
                &bin_path,
                &golgi_sock_path,
            )?;

            let (out, err) = guard.take_pipes();
            let id = format!("w-{}", i);
            vacuole.attach(id.clone(), out, err);

            let v = vacuole.clone();
            // We must subscribe to the broadcast channel for THIS specific task
            let rx = shutdown_tx.subscribe();

            // Pass 'rx' as the 3rd argument
            monitors.spawn(monitor_child(guard, LogTarget::Vacuole(v, id), rx));
        }
        routes.insert(
            traits.name.clone(),
            Target::LocalColony(Arc::new(worker_sockets)),
        );
    } else {
        // Single Cell Mode
        let cell_sock = run_dir.join("cell.sock");
        let log_path = run_dir.join("service.log");
        let monitor_log_path = log_path.clone();

        let guard = nucleus::activate(
            &cell_sock,
            nucleus::LogStrategy::File(log_path),
            &bin_path,
            &golgi_sock_path,
        )?;

        // Subscribe here as well
        let rx = shutdown_tx.subscribe();
        monitors.spawn(monitor_child(guard, LogTarget::File(monitor_log_path), rx));

        routes.insert(traits.name.clone(), Target::GapJunction(cell_sock));
    }

    for (name, path) in &dna.junctions {
        routes.insert(
            name.clone(),
            Target::GapJunction(dir.join(path).join("run/cell.sock")),
        );
    }
    for (name, addr) in &dna.axons {
        let clean = addr.replace("axon://", "");
        routes.insert(
            name.clone(),
            Target::AxonCluster(vec![cell_cli::golgi::AxonTerminal {
                id: "static".into(),
                addr: clean,
                rtt: Duration::from_secs(1),
                last_seen: Instant::now(),
                is_donor: false, // Static configs default to false for now
            }]),
        );
    }

    let golgi = Golgi::new(
        traits.name.clone(),
        &run_dir,
        traits.listen.clone(),
        routes,
        is_donor, // Pass donor flag to router
    )?;

    tokio::select! {
        res = golgi.run() => {
            if let Err(e) = res { sys_log("CRITICAL", &format!("Golgi crashed: {}", e)); }
            let _ = shutdown_tx.send(()); // Kill children if router dies
        },
        _ = tokio::signal::ctrl_c() => {
            sys_log("INFO", "Apoptosis triggered. Shutting down cells...");

            // 1. Broadcast Kill Signal
            let _ = shutdown_tx.send(());

            // 2. Wait for all monitors to finish cleanup
            while let Some(_) = monitors.join_next().await {}

            sys_log("INFO", "Shutdown complete.");
        }
    }
    Ok(())
}

// Monitoring Logic

enum LogTarget {
    Vacuole(Arc<vacuole::Vacuole>, String),
    File(PathBuf),
}

async fn monitor_child(
    mut guard: nucleus::ChildGuard,
    target: LogTarget,
    mut shutdown_rx: broadcast::Receiver<()>,
) {
    let status_msg;

    tokio::select! {
        // Case A: Child exits naturally (crash or finish)
        res = guard.wait() => {
            match res {
                Ok(stats) => {
                    // stats is of type nucleus::Metabolism
                    if stats.exit_code == Some(0) {
                        status_msg = format!(
                            "Process exited cleanly. CPU: {}ms, RAM: {}KB, Wall: {}ms",
                            stats.cpu_time_ms,
                            stats.max_rss_kb,
                            stats.wall_time_ms
                        );
                    } else {
                        status_msg = format!(
                            "CRITICAL: Process crashed. Code: {:?}. CPU: {}ms",
                            stats.exit_code,
                            stats.cpu_time_ms
                        );
                    }
                }
                Err(e) => {
                    status_msg = format!("Supervisor Error: Failed to wait on child: {}", e);
                }
            }
        }

        // Case B: Parent requests shutdown (Ctrl+C)
        _ = shutdown_rx.recv() => {
            // 1. Trigger Kill
            let _ = guard.kill().await;

            // 2. We MUST still wait() to reap the zombie and get final stats
            // Since we updated nucleus.rs, this wait() also returns Metabolism
            match guard.wait().await {
                Ok(stats) => {
                    status_msg = format!(
                        "Shutdown by Supervisor. Partial CPU: {}ms",
                        stats.cpu_time_ms
                    );
                }
                Err(e) => {
                    status_msg = format!("Shutdown by Supervisor (Wait failed: {})", e);
                }
            }
        }
    }

    // Log the result to disk/console
    match target {
        LogTarget::Vacuole(v, id) => {
            v.log(&id, &status_msg).await;
        }
        LogTarget::File(path) => {
            if let Ok(mut file) = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .await
            {
                let timestamp = humantime::format_rfc3339_seconds(SystemTime::now());
                let line = format!("[{}] [SUPERVISOR] {}\n", timestamp, status_msg);
                let _ = file.write_all(line.as_bytes()).await;
            }
        }
    }
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
        let addr = resolve_address(name, raw_addr).await?;

        sys_log(
            "INFO",
            &format!("Snapshotting schema from {} ({})", name, addr),
        );

        let mut connected = false;
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
