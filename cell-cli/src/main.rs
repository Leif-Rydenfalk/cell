use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};
use tokio::net::TcpStream;

// Import from internal lib
use cell_cli::golgi::{Golgi, Target};
use cell_cli::{antigens, nucleus, synapse};

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

#[derive(Deserialize, Debug)]
struct Genome {
    genome: Option<CellTraits>,
    #[serde(default)]
    axons: HashMap<String, String>,
    #[serde(default)]
    junctions: HashMap<String, String>,
    workspace: Option<WorkspaceTraits>,
}

#[derive(Deserialize, Debug)]
struct WorkspaceTraits {
    members: Vec<String>,
}

#[derive(Deserialize, Debug)]
struct CellTraits {
    name: String,
    #[serde(default)]
    listen: Option<String>,
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

        // We use a vector to hold the children.
        // When this vector is dropped (at the end of function), the children are killed.
        let mut children = Vec::new();

        for member in ws.members {
            let member_path = dir.join(&member);

            // Stagger start
            tokio::time::sleep(Duration::from_millis(250)).await;

            // Use tokio::process::Command for async control + kill_on_drop
            let mut cmd = tokio::process::Command::new(&self_exe);
            cmd.arg("mitosis").arg(member_path);
            cmd.kill_on_drop(true); // <--- THIS IS THE FIX
            cmd.stdout(Stdio::inherit());
            cmd.stderr(Stdio::inherit());

            let child = cmd.spawn().context("Failed to spawn member")?;
            children.push(child);
        }

        sys_log("INFO", "System Running. Press Ctrl+C to shutdown.");
        tokio::signal::ctrl_c().await?;

        sys_log("WARN", "Shutdown Signal Received. Terminating all cells...");
        // 'children' goes out of scope here, sending SIGKILL to all members.
        return Ok(());
    }

    // --- CELL MODE (SINGLE NODE) ---
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
    std::fs::create_dir_all(&run_dir)?;

    let mut routes = HashMap::new();

    // --- CRITICAL: SELF-ROUTE ---
    // The node MUST know how to reach its own internal binary.
    // DO NOT REMOVE THIS.
    routes.insert(
        traits.name.clone(),
        Target::GapJunction(run_dir.join("cell.sock")),
    );

    // --- LOCAL JUNCTIONS ---
    for (name, path) in dna.junctions {
        routes.insert(
            name,
            Target::GapJunction(dir.join(path).join("run/cell.sock")),
        );
    }

    // --- REMOTE AXONS (STATIC ROUTES) ---
    // To test Auto-Discovery, we COMMENT OUT this loop.
    // This forces the node to rely on Pheromones to find remote peers.

    /*
    for (name, addr) in dna.axons {
        routes.insert(name, Target::Axon(addr.replace("axon://", "")));
    }
    */

    // Initialize Golgi
    let golgi = Golgi::new(traits.name.clone(), &run_dir, traits.listen.clone(), routes)?;

    let golgi_sock = run_dir.join("golgi.sock");

    // 4. Run Golgi (Network)
    let golgi_handle = tokio::spawn(async move {
        if let Err(e) = golgi.run().await {
            sys_log("CRITICAL", &format!("Golgi failure: {}", e));
        }
    });

    // 5. Run Nucleus (Binary)
    // _guard ensures that when this variable dies, the process dies.
    let _guard = nucleus::activate(&run_dir.join("cell.sock"), &bin_path, &golgi_sock)?;

    sys_log("INFO", &format!("Cell '{}' is operational.", traits.name));

    tokio::select! {
        _ = golgi_handle => {},
        _ = tokio::signal::ctrl_c() => sys_log("INFO", "Apoptosis triggered..."),
    }

    // _guard drops here -> ChildGuard::drop -> kill() -> wait()
    Ok(())
}

async fn snapshot_genomes(root: &Path, axons: &HashMap<String, String>) -> Result<()> {
    let schema_dir = root.join(".cell-genomes");
    std::fs::create_dir_all(&schema_dir)?;

    // Use a temp file for the builder's identity to avoid polluting the real identity
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

        // Retry Loop
        while start.elapsed() < Duration::from_secs(30) {
            match TcpStream::connect(&clean_addr).await {
                Ok(stream) => {
                    // 1. Handshake
                    match synapse::connect_secure(stream, &identity.keypair, true).await {
                        Ok((mut secure_stream, _)) => {
                            // 2. Send Connect Request
                            let mut buf = vec![0u8; 4096];
                            let mut payload = vec![0x01];
                            payload.extend(&(name.len() as u32).to_be_bytes());
                            payload.extend(name.as_bytes());

                            // Encrypt
                            let len = secure_stream
                                .state
                                .write_message(&payload, &mut buf)
                                .unwrap();

                            // Send
                            if let Err(e) =
                                synapse::write_frame(&mut secure_stream.inner, &buf[..len]).await
                            {
                                sys_log("DEBUG", &format!("Failed to send request frame: {}", e));
                                continue;
                            }

                            // 3. Wait for ACK
                            match synapse::read_frame(&mut secure_stream.inner).await {
                                Ok(frame) => {
                                    match secure_stream.state.read_message(&frame, &mut buf) {
                                        Ok(len) => {
                                            if len > 0 && buf[0] == 0x00 {
                                                // 4. Request Schema
                                                let req = b"__GENOME__";
                                                let mut vesicle =
                                                    (req.len() as u32).to_be_bytes().to_vec();
                                                vesicle.extend_from_slice(req);

                                                let len = secure_stream
                                                    .state
                                                    .write_message(&vesicle, &mut buf)
                                                    .unwrap();
                                                synapse::write_frame(
                                                    &mut secure_stream.inner,
                                                    &buf[..len],
                                                )
                                                .await
                                                .unwrap();

                                                // 5. Read Response
                                                let frame =
                                                    synapse::read_frame(&mut secure_stream.inner)
                                                        .await
                                                        .unwrap();
                                                let len = secure_stream
                                                    .state
                                                    .read_message(&frame, &mut buf)
                                                    .unwrap();

                                                if len >= 4 {
                                                    let json_len = u32::from_be_bytes(
                                                        buf[0..4].try_into().unwrap(),
                                                    )
                                                        as usize;
                                                    if len >= 4 + json_len {
                                                        let schema_json = &buf[4..4 + json_len];
                                                        std::fs::write(
                                                            schema_dir
                                                                .join(format!("{}.json", name)),
                                                            schema_json,
                                                        )?;
                                                        sys_log(
                                                            "INFO",
                                                            &format!(
                                                                "SUCCESS: Saved schema for {}",
                                                                name
                                                            ),
                                                        );
                                                        connected = true;
                                                        break;
                                                    }
                                                }
                                            } else {
                                                // Got NACK (0xFF) or garbage
                                                sys_log("WARN", &format!("Remote refused connection to service '{}' (NACK)", name));
                                            }
                                        }
                                        Err(e) => sys_log(
                                            "DEBUG",
                                            &format!("Decryption failed during ACK: {}", e),
                                        ),
                                    }
                                }
                                Err(e) => {
                                    sys_log("DEBUG", &format!("Failed to read ACK frame: {}", e))
                                }
                            }
                        }
                        Err(e) => sys_log("DEBUG", &format!("Handshake failed: {}", e)),
                    }
                }
                Err(e) => {
                    // Only log connection refused once every few seconds to avoid spam
                    if start.elapsed().as_secs() % 5 == 0 {
                        sys_log("DEBUG", &format!("Connection failed: {}", e));
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }

        if !connected {
            sys_log(
                "ERROR",
                &format!("TIMEOUT: Could not fetch schema from {}.", name),
            );
        }
    }
    Ok(())
}
