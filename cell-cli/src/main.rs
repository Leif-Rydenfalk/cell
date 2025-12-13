// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::{anyhow, Context, Result};
use cell_model::protocol::{MitosisRequest, MitosisResponse, TestEvent};
use cell_model::rkyv::Deserialize;
use clap::{Parser, Subcommand};
use colored::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use std::fs;
use cell_sdk::cell_remote;

// === SYSTEM CELLS ===
// We declare them here just like any user cell.
cell_remote!(Nucleus = "nucleus");
cell_remote!(Observer = "observer");

#[derive(Parser)]
#[command(name = "cell")]
#[command(about = "Cell Substrate Control Interface", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Spawn a cell
    Spawn { name: String },
    /// Run tests using the distributed Cell Test System
    Test {
        target: String,
        #[arg(short, long)]
        filter: Option<String>,
    },
    /// Apply a mesh manifest
    Up { 
        #[arg(short, long)]
        file: String 
    },
    /// Live TUI dashboard
    Top,
    /// Follow distributed traces
    Logs { target: String },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Spawn { name } => spawn_cell(name).await,
        Commands::Test { target, filter } => run_test(target, filter).await,
        Commands::Up { file } => apply_manifest(file).await,
        Commands::Top => run_top().await,
        Commands::Logs { target } => tail_logs(target).await,
    }
}

async fn connect_daemon() -> Result<UnixStream> {
    let home = dirs::home_dir().expect("No HOME");
    let socket_path = if let Ok(p) = std::env::var("CELL_SOCKET_DIR") {
        std::path::PathBuf::from(p).join("mitosis.sock")
    } else {
        home.join(".cell/runtime/system/mitosis.sock")
    };

    UnixStream::connect(&socket_path).await.context(format!(
        "Could not connect to Hypervisor at {:?}. Is the system running?",
        socket_path
    ))
}

async fn spawn_cell(name: String) -> Result<()> {
    let mut stream = connect_daemon().await?;
    let req = MitosisRequest::Spawn {
        cell_name: name.clone(),
        config: None,
    };

    send_request(&mut stream, &req).await?;

    let resp: MitosisResponse = recv_response(&mut stream).await?;
    match resp {
        MitosisResponse::Ok { socket_path } => {
            println!("{} Spawned {} at {}", "✔".green(), name, socket_path)
        }
        MitosisResponse::Denied { reason } => println!("{} Spawn failed: {}", "✘".red(), reason),
    }
    Ok(())
}

async fn run_test(target: String, filter: Option<String>) -> Result<()> {
    println!("{} Connecting to Substrate...", "Cell".blue().bold());
    let mut stream = connect_daemon().await?;

    let req = MitosisRequest::Test {
        target_cell: target.clone(),
        filter,
    };
    send_request(&mut stream, &req).await?;

    println!(
        "{} Signaling test run for '{}' on distributed cluster...",
        "Cell".blue().bold(),
        target
    );
    println!("");

    let mut total_passed = 0;
    let mut total_failed = 0;

    loop {
        // Read stream of TestEvents from Hypervisor
        let mut len_buf = [0u8; 4];
        if stream.read_exact(&mut len_buf).await.is_err() {
            break; // EOF
        }
        let len = u32::from_le_bytes(len_buf) as usize;
        let mut buf = vec![0u8; len];
        stream.read_exact(&mut buf).await?;

        let event: TestEvent = cell_model::rkyv::check_archived_root::<TestEvent>(&buf)
            .map_err(|e| anyhow!("Protocol Violation: {:?}", e))?
            .deserialize(&mut cell_model::rkyv::Infallible)
            .unwrap();

        match event {
            TestEvent::Log(msg) => println!("  {}", msg.dimmed()),
            TestEvent::CaseStarted(name) => {
                // print!("{} {} ... ", "RUN".yellow(), name);
            }
            TestEvent::CaseFinished {
                name,
                success,
                duration_ms,
            } => {
                if success {
                    println!(" {} {} ({}ms)", "✔".green(), name, duration_ms);
                    total_passed += 1;
                } else {
                    println!(" {} {} ({}ms)", "✘".red().bold(), name, duration_ms);
                    total_failed += 1;
                }
            }
            TestEvent::SuiteFinished {
                total,
                passed,
                failed,
            } => {
                println!("");
                if failed > 0 {
                    println!("{} Test Suite Failed", "✘".red().bold());
                } else {
                    println!("{} Test Suite Passed", "✔".green().bold());
                }
                println!("  Total:   {}", total);
                println!("  Passed:  {}", passed.to_string().green());
                println!("  Failed:  {}", failed.to_string().red().bold());

                if failed > 0 {
                    std::process::exit(1);
                }
                break;
            }
            TestEvent::Error(e) => {
                println!("{} {}", "ERROR:".red().bold(), e);
                std::process::exit(1);
            }
        }
    }
    Ok(())
}

async fn apply_manifest(path: String) -> Result<()> {
    let yaml = fs::read_to_string(&path).context("Failed to read manifest")?;
    
    // Connect to Nucleus using the generated client
    let mut nucleus = Nucleus::Client::connect().await.context("Nucleus unreachable")?;
    
    println!("{} Applying manifest from {}...", "→".blue(), path);
    
    // Use the RPC method generated by cell_remote!
    let success = nucleus.apply(Nucleus::ApplyManifest { yaml }).await?;
    
    if success {
        println!("{} Mesh converged.", "✔".green());
    } else {
        println!("{} Failed to apply manifest.", "✘".red());
    }
    Ok(())
}

async fn run_top() -> Result<()> {
    // Connect to Nucleus
    let mut nucleus = Nucleus::Client::connect().await.context("Nucleus unreachable")?;

    print!("\x1B[2J\x1B[1;1H"); // Clear screen
    loop {
        let status = nucleus.status().await?;
        
        print!("\x1B[2J\x1B[1;1H"); // Clear screen
        println!("CELL TOP - Lattice Status (Uptime: {}s)", status.uptime_secs);
        println!("-------------------------");
        println!("{:<20} | {:<10}", "Cell", "Status");
        println!("-------------------------");
        
        for cell in status.managed_cells {
            println!("{:<20} | Online", cell);
        }
        
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

async fn tail_logs(target: String) -> Result<()> {
    let mut observer = Observer::Client::connect().await.context("Observer unreachable")?;
    println!("{} Tailing logs for {}...", "→".blue(), target);
    
    loop {
        // Poll for logs
        let logs = observer.tail(10).await?;
        for entry in logs {
            // Filter locally if needed
            if entry.span.service.contains(&target) {
                println!("[{}] {} ({}us)", entry.span.trace_id, entry.span.name, entry.span.duration_us);
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

async fn send_request<
    T: cell_model::rkyv::Serialize<cell_model::rkyv::ser::serializers::AllocSerializer<256>>,
>(
    stream: &mut UnixStream,
    req: &T,
) -> Result<()> {
    let bytes = cell_model::rkyv::to_bytes::<_, 256>(req)?.into_vec();
    stream
        .write_all(&(bytes.len() as u32).to_le_bytes())
        .await?;
    stream.write_all(&bytes).await?;
    Ok(())
}

async fn recv_response<T: cell_model::rkyv::Archive>(stream: &mut UnixStream) -> Result<T>
where
    T::Archived: cell_model::rkyv::Deserialize<T, cell_model::rkyv::Infallible>
        + for<'a> cell_model::rkyv::CheckBytes<
            cell_model::rkyv::validation::validators::DefaultValidator<'a>,
        >,
{
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;

    let archived = cell_model::rkyv::check_archived_root::<T>(&buf)
        .map_err(|e| anyhow::anyhow!("Protocol error: {:?}", e))?;
    Ok(archived
        .deserialize(&mut cell_model::rkyv::Infallible)
        .unwrap())
}