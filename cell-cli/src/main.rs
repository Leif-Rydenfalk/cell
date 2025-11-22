use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc; // Added for Golgi Arc optimization
use std::time::SystemTime;
use tokio::net::TcpStream;

// Import from internal lib
use cell_cli::golgi::{Golgi, Target};
use cell_cli::{antigens, nucleus, synapse, vacuole};

#[derive(Parser)]
#[command(name = "membrane")]
#[command(about = "Cellular Infrastructure Node Manager", long_about = None)]
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

fn sys_log(level: &str, msg: &str) {
    let timestamp = humantime::format_rfc3339_seconds(SystemTime::now());
    eprintln!("[{}] [{}] [MEMBRANE] {}", timestamp, level, msg);
}

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

    sys_log(
        "INFO",
        &format!("Reading genome from {}", genome_path.display()),
    );

    let txt = std::fs::read_to_string(&genome_path).context("Missing genome.toml")?;
    let dna: Genome = toml::from_str(&txt).context("Corrupt DNA (Invalid TOML)")?;

    // --- WORKSPACE MODE (ORCHESTRATOR) ---
    if let Some(ws) = dna.workspace {
        sys_log("INFO", "System detected. Commencing Multi-Cell Mitosis...");
        let self_exe = std::env::current_exe()?;

        let mut children = Vec::new();

        for member in ws.members {
            let member_path = dir.join(&member);
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;

            let mut cmd = tokio::process::Command::new(&self_exe);
            cmd.arg("mitosis").arg(member_path);
            cmd.kill_on_drop(true);
            cmd.stdout(Stdio::inherit());
            cmd.stderr(Stdio::inherit());

            let child = cmd.spawn().context("Failed to spawn member")?;
            children.push(child);
        }

        sys_log("INFO", "System Running. Press Ctrl+C to shutdown.");
        tokio::signal::ctrl_c().await?;
        return Ok(());
    }

    // --- CELL MODE ---
    let traits = dna.genome.context("Invalid genome")?;

    // 1. Snapshot Remote Dependencies
    if !dna.axons.is_empty() {
        snapshot_genomes(&dir, &dna.axons).await?;
    }

    // 2. Build & Locate Binary
    sys_log(
        "INFO",
        &format!("Synthesizing proteins for {}...", traits.name),
    );

    let output = Command::new("cargo")
        .args(&["build", "--release", "--message-format=json"])
        .current_dir(&dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output()?;

    if !output.status.success() {
        anyhow::bail!("Protein synthesis failed.");
    }

    let reader = std::io::BufReader::new(output.stdout.as_slice());
    let mut bin_path: Option<PathBuf> = None;

    use std::io::BufRead;
    for line in reader.lines() {
        if let Ok(l) = line {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&l) {
                if val["reason"] == "compiler-artifact" && val["target"]["name"] == traits.name {
                    if let Some(executable) = val["executable"].as_str() {
                        bin_path = Some(PathBuf::from(executable));
                    }
                }
            }
        }
    }
    let bin_path = bin_path.ok_or_else(|| anyhow!("Could not locate binary"))?;

    let run_dir = dir.join("run");
    if run_dir.exists() {
        std::fs::remove_dir_all(&run_dir)?;
    }
    std::fs::create_dir_all(&run_dir)?;

    let mut routes = HashMap::new();

    // --- COLONY / REPLICA LOGIC ---
    let replicas = traits.replicas.unwrap_or(1);
    let mut child_guards = Vec::new();
    let golgi_sock_path = run_dir.join("golgi.sock");

    if replicas > 1 {
        sys_log("INFO", &format!("Spawning Colony: {} workers.", replicas));

        let socket_dir = run_dir.join("sockets");
        std::fs::create_dir_all(&socket_dir)?;

        // Setup Vacuole (Shared Logging)
        let log_path = run_dir.join("service.log");
        let vacuole = vacuole::Vacuole::new(log_path).await?;

        let mut worker_sockets = Vec::new();

        for i in 0..replicas {
            // Use subdirectory for isolation of socket (prevents name collisions)
            let worker_dir = run_dir.join("workers").join(i.to_string());
            std::fs::create_dir_all(&worker_dir)?;
            let sock_path = worker_dir.join("cell.sock");

            worker_sockets.push(sock_path.clone());

            // LogStrategy::Piped -> streams back to parent -> Vacuole
            let mut guard = nucleus::activate(
                &sock_path,
                nucleus::LogStrategy::Piped,
                &bin_path,
                &golgi_sock_path,
            )?;

            // Attach pipes to Vacuole
            let (out, err) = guard.take_pipes();
            vacuole.attach(format!("w-{}", i), out, err);

            child_guards.push(guard);
        }

        routes.insert(
            traits.name.clone(),
            Target::LocalColony(Arc::new(worker_sockets)), // Wrap in Arc for Golgi
        );
    } else {
        // Single Cell Mode (Direct File Logging)
        let cell_sock = run_dir.join("cell.sock");
        let log_path = run_dir.join("service.log");

        let guard = nucleus::activate(
            &cell_sock,
            nucleus::LogStrategy::File(log_path),
            &bin_path,
            &golgi_sock_path,
        )?;
        child_guards.push(guard);

        routes.insert(traits.name.clone(), Target::GapJunction(cell_sock));
    }

    // --- LOCAL JUNCTIONS ---
    for (name, path) in dna.junctions {
        routes.insert(
            name,
            Target::GapJunction(dir.join(path).join("run/cell.sock")),
        );
    }

    // --- REMOTE AXONS (STATIC ROUTES) ---
    for (name, addr) in dna.axons {
        let clean = addr.replace("axon://", "");
        routes.insert(
            name,
            Target::AxonCluster(vec![cell_cli::golgi::AxonTerminal {
                id: "static".into(),
                addr: clean,
                rtt: std::time::Duration::from_secs(1),
                last_seen: std::time::Instant::now(),
            }]),
        );
    }

    // Initialize Golgi
    let golgi = Golgi::new(traits.name.clone(), &run_dir, traits.listen.clone(), routes)?;

    sys_log(
        "INFO",
        &format!("Cell '{}' (or Colony) is operational.", traits.name),
    );

    tokio::select! {
        res = golgi.run() => {
            if let Err(e) = res {
                sys_log("CRITICAL", &format!("Golgi failure: {}", e));
            }
        },
        _ = tokio::signal::ctrl_c() => sys_log("INFO", "Apoptosis triggered..."),
    }

    Ok(())
}

async fn snapshot_genomes(root: &Path, axons: &HashMap<String, String>) -> Result<()> {
    let schema_dir = root.join(".cell-genomes");
    std::fs::create_dir_all(&schema_dir)?;
    let temp_id_path = root.join("run/temp_builder_identity");
    let identity = antigens::Antigens::load_or_create(temp_id_path)?;

    for (name, addr) in axons {
        let clean_addr = addr.replace("axon://", "");
        sys_log(
            "INFO",
            &format!("Fetching schema from {} ({})", name, clean_addr),
        );

        let start = std::time::Instant::now();
        let mut connected = false;

        while start.elapsed() < std::time::Duration::from_secs(10) {
            if let Ok(stream) = TcpStream::connect(&clean_addr).await {
                if let Ok((mut secure, _)) =
                    synapse::connect_secure(stream, &identity.keypair, true).await
                {
                    let mut buf = vec![0u8; 4096];
                    // Connect Frame
                    let mut payload = vec![0x01];
                    payload.extend(&(name.len() as u32).to_be_bytes());
                    payload.extend(name.as_bytes());
                    let len = secure.state.write_message(&payload, &mut buf).unwrap();
                    synapse::write_frame(&mut secure.inner, &buf[..len]).await?;

                    // Read ACK
                    let frame = synapse::read_frame(&mut secure.inner).await?;
                    let len = secure.state.read_message(&frame, &mut buf)?;
                    if len > 0 && buf[0] == 0x00 {
                        // Fetch Genome
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
                                break;
                            }
                        }
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        if !connected {
            sys_log("WARN", &format!("Could not fetch schema for {}", name));
        }
    }
    Ok(())
}
