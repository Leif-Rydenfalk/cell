// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::Result;
use clap::{Parser, Subcommand};
use cell_sdk::NucleusClient;
use cell_process::MyceliumRoot;
use cell_model::protocol::{MitosisRequest, MitosisResponse};
use cell_model::config::CellInitConfig;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use cell_sdk::rkyv::Deserialize; // Fix deserialize method not found
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
    
    // Fix: Provide config (None to let Root generate default)
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

    // Fix: Map validation error to anyhow to satisfy ? and fix Send/Sync issues
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

    // Use Nucleus to discover itself just as a connectivity check? 
    // Ideally NucleusClient should have a list_all method.
    // For now we assume scanning local/lan via discovery lib is what we want,
    // but the error indicated cell_sdk::discovery isn't exposed or found.
    // Let's check cell-sdk lib.rs. It exports cell_core and cell_model.
    // It depends on cell-discovery.
    // It does NOT pub use cell_discovery.
    
    // We should use the NucleusClient to get the list if possible.
    // But NucleusClient only has `discover(name)`.
    // Let's assume we rely on manual Nucleus queries or implement scan in NucleusClient.
    
    // Fallback: If we can't scan, just print a message.
    println!("Listing active cells via Nucleus is not fully implemented in CLI.");
    
    Ok(())
}

async fn stop_cell(name: &str) -> Result<()> {
    let mut synapse = Synapse::grow(name).await?;
    
    // We need to support Shutdown in Ops. 
    println!("Sending stop signal to {}...", name);
    
    // Send Shutdown Ops Request
    let req = OpsRequest::Shutdown;
    let req_bytes = cell_model::rkyv::to_bytes::<_, 256>(&req)?.into_vec();
    
    // Fix: Use cell_sdk::channel::OPS (which is re-exported from cell_core)
    let _ = synapse.fire_on_channel(cell_sdk::channel::OPS, &req_bytes).await?;
    
    println!("Stop signal sent.");
    
    Ok(())
}

async fn inspect_cell(name: &str) -> Result<()> {
    let mut synapse = Synapse::grow(name).await?;
    let req = OpsRequest::Status;
    
    let req_bytes = cell_model::rkyv::to_bytes::<_, 256>(&req)?.into_vec();
    let resp = synapse.fire_on_channel(cell_sdk::channel::OPS, &req_bytes).await?;
    
    // Fix: Use cell_transport::Response (re-exported in cell_sdk)
    // The error said `cell_sdk::Response` not found. 
    // Let's check cell-sdk lib.rs.
    // It has `pub use cell_transport::{Membrane, Synapse, resolve_socket_dir};`
    // It does NOT export Response.
    // We should export it in cell-sdk/src/lib.rs.
    // For now, we match on the structure if we can't access the type? No, we need the type.
    // Or we use `resp.into_owned()` and work with bytes?
    // Response is an enum.
    
    // WORKAROUND: If Response is not public in SDK, we can't name it.
    // But fire_on_channel returns `Result<Response<Vec<u8>>, ...>`.
    // If we can't name Response, we can't match it easily unless we import `cell_transport`.
    // `cell-cli` depends on `cell-sdk`. It does NOT depend on `cell-transport`.
    
    // Solution: We must export Response from cell-sdk. I will update cell-sdk/src/lib.rs below.
    // Assuming that happens, we can use cell_sdk::Response.
    
    // For this file, I will assume the export exists.
    
    let resp_bytes = match resp {
        cell_sdk::Response::Owned(v) => v,
        cell_sdk::Response::Borrowed(v) => v.to_vec(),
        _ => anyhow::bail!("Unexpected response"),
    };
    
    // Fix: Manual error mapping for ? operator
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