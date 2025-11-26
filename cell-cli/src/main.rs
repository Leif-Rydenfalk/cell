use anyhow::{anyhow, Context, Result};
use cell_cli::genesis::run_genesis;
use clap::{Parser, Subcommand};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::Mutex;

// Import from internal lib
use cell_cli::config::Genome;
use cell_cli::golgi::pheromones;
use cell_cli::{antigens, mitochondria, synapse, sys_log};

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
        #[arg(long)]
        donor: bool,
    },
    /// Manage financial resources (ATP).
    Wallet { dir: PathBuf },
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
        if !dir.join("mitochondria.json").exists() {
            anyhow::bail!(
                "No runtime data found at {}. Have you run 'mitosis' yet?",
                dir.display()
            );
        }
    }

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
    let genome_path = dir.join("Cell.toml");

    sys_log("INFO", "Scanning workspace for cellular life...");
    let mut registry = CellRegistry::new();
    inventory_cells(&dir, &mut registry)?;
    sys_log(
        "INFO",
        &format!("Discovered {} local cells.", registry.len()),
    );

    let txt = std::fs::read_to_string(&genome_path).context("Missing Cell.toml")?;
    let dna: Genome = toml::from_str(&txt)?;
    let running = Arc::new(Mutex::new(HashSet::new()));

    if let Some(ws) = dna.workspace {
        sys_log("INFO", "Workspace detected. Resolving dependency graph...");
        for member_dir in ws.members {
            let path = dir.join(member_dir);
            let m_txt = std::fs::read_to_string(path.join("Cell.toml"))?;
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
        // Single Cell Boot
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

    let txt = std::fs::read_to_string(cell_path.join("Cell.toml"))?;
    let dna: Genome = toml::from_str(&txt)?;

    // Extract traits to check for runner/binary
    let traits = dna
        .genome
        .as_ref()
        .ok_or_else(|| anyhow!("Invalid genome"))?;

    sys_log("INFO", &format!("[{}] Checking dependencies...", cell_name));

    for (axon_name, axon_addr) in &dna.axons {
        let target_addr = resolve_address(axon_name, axon_addr).await;

        match target_addr {
            Ok(addr) => {
                if verify_tcp(&addr).await.is_ok() {
                    sys_log(
                        "INFO",
                        &format!(
                            "[{}] Dependency '{}' found at {}",
                            cell_name, axon_name, addr
                        ),
                    );
                    continue;
                }
            }
            Err(_) => {}
        }

        if registry.contains_key(axon_name) {
            sys_log(
                "WARN",
                &format!(
                    "[{}] Dependency '{}' not reachable. Booting local instance...",
                    cell_name, axon_name
                ),
            );
            Box::pin(ensure_active(
                axon_name,
                registry,
                running.clone(),
                false,
                is_donor,
            ))
            .await?;

            let mut attempts = 0;
            loop {
                if let Ok(addr) = resolve_address(axon_name, axon_addr).await {
                    if verify_tcp(&addr).await.is_ok() {
                        break;
                    }
                }
                attempts += 1;

                if attempts == 3 {
                    sys_log("WARN", &format!("Dependency '{}' is taking a long time. Check run/service.log for crashes.", axon_name));
                }

                if attempts > 10 {
                    anyhow::bail!("Dependency '{}' failed to boot (Timeout).", axon_name);
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

    snapshot_genomes(cell_path, &dna.axons).await?;

    // --- LOGIC SPLIT: Interpreted vs Compiled ---
    let bin_path = if traits.runner.is_some() {
        // INTERPRETED MODE
        sys_log(
            "INFO",
            &format!("[{}] Interpreted Mode. Skipping compilation.", cell_name),
        );

        let script_name = traits.binary.as_ref().ok_or_else(|| {
            anyhow!("'binary' field (script path) is required for interpreted cells.")
        })?;

        let script_path = cell_path.join(script_name);
        if !script_path.exists() {
            anyhow::bail!("Script not found at: {}", script_path.display());
        }
        script_path
    } else {
        // COMPILED MODE
        sys_log("INFO", &format!("[{}] Compiling...", cell_name));
        compile_cell(cell_path, cell_name)?
    };

    if is_root {
        sys_log(
            "INFO",
            &format!("[{}] Launching Daemon (Foreground)...", cell_name),
        );
        launch_daemon_foreground(cell_path, &bin_path, is_donor).await?;
    } else {
        sys_log(
            "INFO",
            &format!("[{}] Spawning Daemon (Background)...", cell_name),
        );
        spawn_daemon_background(cell_path, &bin_path, is_donor)?;
    }

    Ok(())
}

async fn resolve_address(name: &str, raw_addr: &str) -> Result<String> {
    let clean = raw_addr.replace("axon://", "");
    if clean.contains(':') {
        if tokio::net::lookup_host(&clean).await.is_ok() {
            return Ok(clean);
        }
    }

    sys_log(
        "INFO",
        &format!("Searching network for '{}' (Pheromones)...", name),
    );

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

// Replaces spawn_cell_background
fn spawn_daemon_background(dir: &Path, bin_path: &Path, is_donor: bool) -> Result<()> {
    let daemon_exe = std::env::current_exe()?
        .parent()
        .unwrap()
        .join("cell-daemon");

    // Check 1: Does the daemon binary exist?
    if !daemon_exe.exists() {
        anyhow::bail!("Binary missing: {}", daemon_exe.display());
    }

    // Check 2: Create the 'run' directory (The Fix for Error 2)
    let run_dir = dir.join("run");
    if !run_dir.exists() {
        std::fs::create_dir_all(&run_dir).context("Failed to create 'run' directory")?;
    }

    // Check 3: Create the log file
    let log_file =
        std::fs::File::create(run_dir.join("daemon.log")).context("Failed to create daemon.log")?;

    let mut cmd = Command::new(daemon_exe);
    cmd.arg(dir).arg("--bin").arg(bin_path);

    if is_donor {
        cmd.arg("--donor");
    }

    cmd.stdout(log_file.try_clone().context("Failed to clone stdout")?)
        .stderr(log_file)
        .spawn()
        .context("Failed to spawn cell-daemon")?;
    Ok(())
}

// Replaces run_cell_runtime logic (which is now in daemon.rs)
async fn launch_daemon_foreground(dir: &Path, bin_path: &Path, is_donor: bool) -> Result<()> {
    let daemon_exe = std::env::current_exe()?
        .parent()
        .unwrap()
        .join("cell-daemon");

    let mut cmd = Command::new(daemon_exe);
    cmd.arg(dir).arg("--bin").arg(bin_path);

    if is_donor {
        cmd.arg("--donor");
    }

    let mut child = cmd.spawn().context("Failed to launch cell-daemon")?;

    // Wait for the daemon to exit
    let status = child.wait()?;
    if !status.success() {
        anyhow::bail!("Daemon exited with status: {}", status);
    }

    Ok(())
}

fn inventory_cells(dir: &Path, registry: &mut CellRegistry) -> Result<()> {
    if dir.is_dir() {
        let genome_file = dir.join("Cell.toml");
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
                let mut payload = vec![0x01];
                payload.extend(&(name.len() as u32).to_be_bytes());
                payload.extend(name.as_bytes());
                let len = secure.state.write_message(&payload, &mut buf).unwrap();
                synapse::write_frame(&mut secure.inner, &buf[..len]).await?;

                let frame = synapse::read_frame(&mut secure.inner).await?;
                let len = secure.state.read_message(&frame, &mut buf)?;
                if len > 0 && buf[0] == 0x00 {
                    let req = b"__GENOME__";
                    let mut v = (req.len() as u32).to_be_bytes().to_vec();
                    v.extend_from_slice(req);
                    let len = secure.state.write_message(&v, &mut buf).unwrap();
                    synapse::write_frame(&mut secure.inner, &buf[..len]).await?;

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
