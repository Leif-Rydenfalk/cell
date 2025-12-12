// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::Result;
use clap::{Parser, Subcommand};
use cell_sdk::NucleusClient;
use cell_process::MyceliumRoot;
use cell_model::protocol::{MitosisRequest, MitosisResponse};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use cell_sdk::rkyv::Deserialize;
use cell_sdk::Synapse;
use cell_model::ops::{OpsRequest, OpsResponse};

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
            std::future::pending::<()>().await;
        }
        Commands::Nucleus => {
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
    
    let req = MitosisRequest::Spawn { 
        cell_name: name.to_string(),
        config: None 
    };
    
    let req_bytes = cell_model::rkyv::to_bytes::<_, 256>(&req)?.into_vec();
    
    stream.write_all(&(req_bytes.len() as u32).to_le_bytes()).await?;
    stream.write_all(&req_bytes).await?;

    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;
    
    let mut resp_buf = vec![0u8; len];
    stream.read_exact(&mut resp_buf).await?;

    let archived = cell_model::rkyv::check_archived_root::<MitosisResponse>(&resp_buf)
        .map_err(|e| anyhow::anyhow!("Invalid response: {:?}", e))?;
        
    let resp: MitosisResponse = archived.deserialize(&mut cell_model::rkyv::Infallible).unwrap();

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
    // Attempt to connect to Nucleus
    if let Ok(_nucleus) = NucleusClient::connect().await {
         // Nucleus Client does not expose list_all yet.
         // Fallback to scanning discovery via SDK re-export if possible, or just stub.
         println!("Listing active cells via Nucleus is not fully implemented in CLI.");
    } else {
         println!("Nucleus not reachable.");
    }
    
    Ok(())
}

async fn stop_cell(name: &str) -> Result<()> {
    let mut synapse = Synapse::grow(name).await?;
    println!("Sending stop signal to {}...", name);
    
    let req = OpsRequest::Shutdown;
    let req_bytes = cell_model::rkyv::to_bytes::<_, 256>(&req)?.into_vec();
    
    let _ = synapse.fire_on_channel(cell_sdk::channel::OPS, &req_bytes).await?;
    
    println!("Stop signal sent.");
    Ok(())
}

async fn inspect_cell(name: &str) -> Result<()> {
    let mut synapse = Synapse::grow(name).await?;
    let req = OpsRequest::Status;
    
    let req_bytes = cell_model::rkyv::to_bytes::<_, 256>(&req)?.into_vec();
    let resp = synapse.fire_on_channel(cell_sdk::channel::OPS, &req_bytes).await?;
    
    let resp_bytes = match resp {
        cell_sdk::Response::Owned(v) => v,
        cell_sdk::Response::Borrowed(v) => v.to_vec(),
        _ => anyhow::bail!("Unexpected response"),
    };
    
    let archived = cell_model::rkyv::check_archived_root::<OpsResponse>(&resp_bytes)
        .map_err(|e| anyhow::anyhow!("Invalid response bytes: {:?}", e))?;
        
    let status: OpsResponse = archived.deserialize(&mut cell_model::rkyv::Infallible).unwrap();
        
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