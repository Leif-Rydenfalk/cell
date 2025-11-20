mod antigens;
mod golgi;
mod nucleus;
mod synapse;

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use golgi::{Golgi, Target};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[derive(Parser)]
#[command(name = "membrane")]
#[command(about = "Cellular Infrastructure Node Manager", long_about = None)]
struct Cli {
    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Compiles code, syncs schemas, and starts the cell node.
    Mitosis { dir: PathBuf },
}

#[derive(Deserialize, Debug)]
struct Genome {
    genome: CellTraits,
    #[serde(default)]
    axons: HashMap<String, String>,
    #[serde(default)]
    junctions: HashMap<String, String>,
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
    
    sys_log("INFO", &format!("Reading genome from {}", genome_path.display()));
    
    let txt = std::fs::read_to_string(&genome_path).context("Missing genome.toml")?;
    let dna: Genome = toml::from_str(&txt).context("Corrupt DNA (Invalid TOML)")?;

    // 1. Snapshot Remote Dependencies (Schema Sync)
    if !dna.axons.is_empty() {
        snapshot_genomes(&dir, &dna.axons).await?;
    }

    // 2. Protein Synthesis (Build)
    sys_log("INFO", "Synthesizing proteins (Compiling)...");
    let status = Command::new("cargo")
        .arg("build")
        .arg("--release")
        .current_dir(&dir)
        .status()
        .context("Cargo execution failed")?;

    if !status.success() {
        anyhow::bail!("Protein synthesis failed. Check compiler output.");
    }

    // 3. Locate Binary
    // Assumes binary name matches cell name. 
    // Production version should parse Cargo.toml to be sure.
    let bin_path = dir.join("target/release").join(&dna.genome.name);
    if !bin_path.exists() {
        anyhow::bail!("Binary not found at {:?}", bin_path);
    }

    // 4. Initialize Golgi (Routing)
    let run_dir = dir.join("run");
    std::fs::create_dir_all(&run_dir)?;

    let mut routes = HashMap::new();
    
    // Self-Route
    routes.insert(dna.genome.name.clone(), Target::GapJunction(run_dir.join("cell.sock")));

    // External Routes
    for (name, addr) in dna.axons {
        // Strip protocol prefix if present
        let clean_addr = addr.replace("axon://", "");
        routes.insert(name, Target::Axon(clean_addr));
    }
    
    // Local Routes
    for (name, path) in dna.junctions {
        let target = dir.join(path).join("run/cell.sock");
        routes.insert(name, Target::GapJunction(target));
    }

    let golgi = Golgi::new(&run_dir, dna.genome.listen.clone(), routes)?;
    let golgi_sock = run_dir.join("golgi.sock");

    // 5. Start Transport Loop
    let golgi_handle = tokio::spawn(async move {
        if let Err(e) = golgi.run().await {
            sys_log("CRITICAL", &format!("Golgi failure: {}", e));
        }
    });

    // 6. Activate Nucleus
    nucleus::activate(&run_dir.join("cell.sock"), &bin_path, &golgi_sock)?;

    sys_log("INFO", "Cell is fully operational. Press Ctrl+C to stop.");

    tokio::select! {
        _ = golgi_handle => {},
        _ = tokio::signal::ctrl_c() => {
            sys_log("INFO", "Apoptosis triggered (Shutting down)...");
        }
    }

    Ok(())
}

/// Connects to remote cells during build time to fetch their schemas
async fn snapshot_genomes(root: &Path, axons: &HashMap<String, String>) -> Result<()> {
    let schema_dir = root.join(".cell-genomes");
    std::fs::create_dir_all(&schema_dir)?;
    
    // We need a temporary identity to handshake for schema fetch
    let identity = antigens::Antigens::load_or_create()?;

    for (name, addr) in axons {
        let clean_addr = addr.replace("axon://", "");
        sys_log("INFO", &format!("Snapshotting genome from {} ({})", name, clean_addr));

        match TcpStream::connect(&clean_addr).await {
            Ok(stream) => {
                // Secure Handshake (Initiator)
                let (mut secure_stream, _) = synapse::connect_secure(stream, &identity.keypair, true).await?;
                
                // Send Request: [0x01] [Len] [Name] (Self-route to get own schema?)
                // Actually, the protocol to get schema is usually a specific OpCode or a reserved name.
                // In `cytosol`, we check for `__GENOME__`.
                // But we need to route that via the remote Golgi to the remote Nucleus.
                
                // 1. Connect to Remote Service via Remote Golgi
                secure_stream.write_all(&[0x01]).await?; // OpCode Connect
                let name_bytes = name.as_bytes();
                secure_stream.write_all(&(name_bytes.len() as u32).to_be_bytes()).await?;
                secure_stream.write_all(name_bytes).await?;

                // 2. Wait for ACK
                let mut ack = [0u8; 1];
                secure_stream.read_exact(&mut ack).await?;
                if ack[0] != 0x00 {
                    sys_log("WARN", &format!("Failed to connect to remote service {}", name));
                    continue;
                }

                // 3. Request Schema
                let req = b"__GENOME__";
                let req_len = (req.len() as u32).to_be_bytes();
                secure_stream.write_all(&req_len).await?;
                secure_stream.write_all(req).await?;

                // 4. Read Response
                let mut len_buf = [0u8; 4];
                secure_stream.read_exact(&mut len_buf).await?;
                let len = u32::from_be_bytes(len_buf) as usize;
                
                let mut schema_buf = vec![0u8; len];
                secure_stream.read_exact(&mut schema_buf).await?;

                std::fs::write(schema_dir.join(format!("{}.json", name)), schema_buf)?;
            }
            Err(e) => {
                sys_log("WARN", &format!("Could not contact {}: {}", name, e));
            }
        }
    }
    Ok(())
}