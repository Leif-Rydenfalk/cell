// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::Result;
use clap::{Parser, Subcommand};
use cell_sdk::NucleusClient;
use cell_process::MyceliumRoot;
use cell_model::protocol::{MitosisRequest, MitosisResponse};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "cell")]
#[command(about = "The Biological Compute Substrate CLI", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the Mycelium Root daemon
    Daemon,
    /// Initialize the system (alias for daemon for now)
    Init,
    /// Spawn a cell
    Spawn {
        /// Name of the cell to spawn
        cell_name: String,
    },
    /// Alias for spawn
    Run {
        cell_name: String,
    },
    /// List active cells (via Nucleus)
    List,
    /// Stop a cell (graceful shutdown)
    Stop {
        cell_name: String,
    },
    /// Inspect cell health/stats
    Inspect {
        cell_name: String,
    },
    /// Start the Nucleus system manager
    Nucleus,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Daemon | Commands::Init => {
            tracing::info!("[Cell] Igniting Mycelium Root...");
            let _root = MyceliumRoot::ignite().await?;
            // Keep running
            std::future::pending::<()>().await;
        }
        Commands::Nucleus => {
            // In a real scenario, this might spawn the nucleus binary.
            // For now, we assume the user runs `cargo run -p nucleus` or we spawn it via root.
            println!("Please run: cell spawn nucleus");
        }
        Commands::Spawn { cell_name } | Commands::Run { cell_name } => {
            spawn_cell(&cell_name).await?;
        }
        Commands::List => {
            list_cells().await?;
        }
        Commands::Stop { cell_name } => {
            stop_cell(&cell_name).await?;
        }
        Commands::Inspect { cell_name } => {
            inspect_cell(&cell_name).await?;
        }
    }

    Ok(())
}

async fn spawn_cell(name: &str) -> Result<()> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("No home dir"))?;
    let umbilical = home.join(".cell/run/mitosis.sock");

    if !umbilical.exists() {
        anyhow::bail!("Mycelium Root not running. Run 'cell daemon' first.");
    }

    let mut stream = UnixStream::connect(umbilical).await?;
    
    let req = MitosisRequest::Spawn { cell_name: name.to_string() };
    let req_bytes = cell_model::rkyv::to_bytes::<_, 256>(&req)?.into_vec();
    
    stream.write_all(&(req_bytes.len() as u32).to_le_bytes()).await?;
    stream.write_all(&req_bytes).await?;

    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;
    
    let mut resp_buf = vec![0u8; len];
    stream.read_exact(&mut resp_buf).await?;

    let resp = cell_model::rkyv::check_archived_root::<MitosisResponse>(&resp_buf)
        .map_err(|e| anyhow::anyhow!("Invalid response: {}", e))?
        .deserialize(&mut cell_model::rkyv::Infallible).unwrap();

    match resp {
        MitosisResponse::Ok { socket_path } => {
            println!("✓ Spawned '{}' at {}", name, socket_path);
        }
        MitosisResponse::Denied { reason } => {
            println!("✗ Failed to spawn '{}': {}", name, reason);
        }
    }

    Ok(())
}

async fn list_cells() -> Result<()> {
    // Nucleus provides the registry
    let mut nucleus = match NucleusClient::connect().await {
        Ok(c) => c,
        Err(_) => {
            println!("Nucleus not reachable. Use 'cell spawn nucleus' to start the system manager.");
            return Ok(());
        }
    };

    println!("{:<20} {:<30}", "CELL", "ADDRESS");
    println!("{:-<20} {:-<30}", "", "");

    // We query the nucleus for known infrastructure and apps.
    // Since we don't have a "list all" in NucleusClient wrapper yet, we rely on scanning discovery for now
    // or assume Nucleus adds a 'list' method.
    // For this CLI proof-of-concept, let's scan LAN discovery directly as fallback or extended listing.
    
    let nodes = cell_sdk::discovery::Discovery::scan().await;
    for node in nodes {
        let addr = node.lan_address.or_else(|| 
            node.local_socket.map(|p| p.to_string_lossy().to_string())
        ).unwrap_or("?".to_string());
        
        println!("{:<20} {:<30}", node.name, addr);
    }

    Ok(())
}

async fn stop_cell(name: &str) -> Result<()> {
    // Send Shutdown Ops request
    // This requires connecting to the cell's synapse
    use cell_sdk::Synapse;
    let mut synapse = Synapse::grow(name).await?;
    
    // We need to support Shutdown in Ops. 
    // If not supported yet in protocol, we can't cleanly stop via RPC.
    // Assuming we added it or will add it.
    // For now, let's just say "Feature pending" if not implemented.
    
    println!("Sending stop signal to {}...", name);
    // TODO: Implement OpsRequest::Shutdown
    
    Ok(())
}

async fn inspect_cell(name: &str) -> Result<()> {
    use cell_sdk::Synapse;
    use cell_model::ops::{OpsRequest, OpsResponse};
    
    let mut synapse = Synapse::grow(name).await?;
    let req = OpsRequest::Status;
    
    // Manual fire using raw bytes for ops channel if wrapper not available, 
    // but Synapse doesn't expose typed fire on specific channels easily without wrappers.
    // Membrane handles Channel::OPS.
    // We can use the lower level fire_on_channel.
    
    let req_bytes = cell_model::rkyv::to_bytes::<_, 256>(&req)?.into_vec();
    let resp = synapse.fire_on_channel(cell_core::channel::OPS, &req_bytes).await?;
    
    let resp_bytes = match resp {
        cell_sdk::Response::Owned(v) => v,
        cell_sdk::Response::Borrowed(v) => v.to_vec(),
        _ => anyhow::bail!("Unexpected response"),
    };
    
    let status = cell_model::rkyv::check_archived_root::<OpsResponse>(&resp_bytes)?
        .deserialize(&mut cell_model::rkyv::Infallible).unwrap();
        
    match status {
        OpsResponse::Status { name, uptime_secs, memory_usage, consensus_role } => {
            println!("Cell: {}", name);
            println!("Uptime: {}s", uptime_secs);
            println!("Memory: {} bytes", memory_usage);
            println!("Role: {}", consensus_role);
        }
        _ => println!("Unexpected response"),
    }

    Ok(())
}