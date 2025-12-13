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
use std::path::{Path, PathBuf};
use std::time::Duration;
use cell_sdk::cell_remote;
use serde::Deserialize as SerdeDeserialize;

// === SYSTEM CELLS ===
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
    /// Start a cell workspace or apply a mesh manifest
    Up { 
        #[arg(short, long)]
        file: Option<String> 
    },
    /// Live TUI dashboard
    Top,
    /// Follow distributed traces
    Logs { target: String },
    /// Garbage collect unused cells
    Prune,
}

#[derive(SerdeDeserialize)]
struct CellWorkspace {
    workspace: WorkspaceConfig,
}

#[derive(SerdeDeserialize)]
struct WorkspaceConfig {
    members: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Spawn { name } => spawn_cell(name).await,
        Commands::Test { target, filter } => run_test(target, filter).await,
        Commands::Up { file } => run_up(file).await,
        Commands::Top => run_top().await,
        Commands::Logs { target } => tail_logs(target).await,
        Commands::Prune => prune_cells().await,
    }
}

// ... (Existing helper functions connect_daemon, spawn_cell, run_up, run_test, apply_manifest, run_top, tail_logs, send_request, recv_response remain same) ...

async fn connect_daemon() -> Result<UnixStream> {
    let home = dirs::home_dir().expect("No HOME");
    let socket_path = if let Ok(p) = std::env::var("CELL_SOCKET_DIR") {
        PathBuf::from(p).join("mitosis.sock")
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
        MitosisResponse::Denied { reason } => return Err(anyhow!("Spawn denied: {}", reason)),
    }
    Ok(())
}

async fn run_up(file: Option<String>) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let toml_path = cwd.join("Cell.toml");
    
    if file.is_none() && toml_path.exists() {
        return run_workspace_up(&toml_path).await;
    }

    let target = file.ok_or_else(|| anyhow!("No Cell.toml found and no file specified"))?;
    
    if target.ends_with(".toml") || target == "Cell.toml" {
        run_workspace_up(Path::new(&target)).await
    } else {
        apply_manifest(target).await
    }
}

async fn run_workspace_up(path: &Path) -> Result<()> {
    println!("{} Reading workspace from {}...", "→".blue(), path.display());
    
    let content = fs::read_to_string(path)?;
    let config: CellWorkspace = toml::from_str(&content)
        .context("Failed to parse Cell.toml")?;

    let root_dir = path.parent().unwrap();
    let registry_dir = dirs::home_dir().expect("No HOME").join(".cell/registry");
    fs::create_dir_all(&registry_dir)?;

    println!("{} Found {} members: {:?}", "ℹ".blue(), config.workspace.members.len(), config.workspace.members);

    for member in &config.workspace.members {
        let member_path = root_dir.join(member);
        if !member_path.exists() {
            println!("{} Warning: Member path {} does not exist", "⚠".yellow(), member_path.display());
            continue;
        }

        let link_path = registry_dir.join(member);
        if link_path.exists() {
            let _ = fs::remove_file(&link_path); 
        }

        #[cfg(unix)]
        std::os::unix::fs::symlink(&member_path, &link_path)
            .context(format!("Failed to link {} to registry", member))?;
            
        println!("   Linked {} -> Registry", member);
    }

    let mut pending = config.workspace.members.clone();
    let mut attempt = 0;
    const MAX_ATTEMPTS: usize = 5;

    while !pending.is_empty() && attempt < MAX_ATTEMPTS {
        attempt += 1;
        if attempt > 1 {
            println!("{} Retrying failed cells (Attempt {}/{})...", "↻".yellow(), attempt, MAX_ATTEMPTS);
            tokio::time::sleep(Duration::from_secs(2)).await;
        }

        let mut next_pending = Vec::new();

        for cell in pending {
            print!("   Spawning {}... ", cell);
            use std::io::Write;
            std::io::stdout().flush()?;

            match spawn_cell(cell.clone()).await {
                Ok(_) => {
                    println!("{}", "OK".green());
                }
                Err(_e) => {
                    println!("{}", "Pending".yellow());
                    next_pending.push(cell);
                }
            }
        }
        pending = next_pending;
    }

    if !pending.is_empty() {
        println!("\n{} Failed to spawn the following cells after {} attempts:", "✘".red(), MAX_ATTEMPTS);
        for cell in pending {
            println!("   - {}", cell);
            if let Err(e) = spawn_cell(cell).await {
                println!("     Error: {}", e);
            }
        }
        return Err(anyhow!("Workspace startup incomplete"));
    }

    println!("\n{} Workspace active.", "✔".green());
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

    let mut _total_passed = 0;
    let mut _total_failed = 0;

    loop {
        let mut len_buf = [0u8; 4];
        if stream.read_exact(&mut len_buf).await.is_err() {
            break;
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
            TestEvent::CaseStarted(_name) => {},
            TestEvent::CaseFinished { name, success, duration_ms } => {
                if success {
                    println!(" {} {} ({}ms)", "✔".green(), name, duration_ms);
                    _total_passed += 1;
                } else {
                    println!(" {} {} ({}ms)", "✘".red().bold(), name, duration_ms);
                    _total_failed += 1;
                }
            }
            TestEvent::SuiteFinished { total, passed, failed } => {
                println!("");
                if failed > 0 {
                    println!("{} Test Suite Failed", "✘".red().bold());
                } else {
                    println!("{} Test Suite Passed", "✔".green().bold());
                }
                println!("  Total:   {}", total);
                println!("  Passed:  {}", passed.to_string().green());
                println!("  Failed:  {}", failed.to_string().red().bold());

                if failed > 0 { std::process::exit(1); }
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
    let mut nucleus = Nucleus::Client::connect().await.context("Nucleus unreachable")?;
    println!("{} Applying manifest from {}...", "→".blue(), path);
    let success = nucleus.apply(Nucleus::ApplyManifest { yaml }).await?;
    if success {
        println!("{} Mesh converged.", "✔".green());
    } else {
        println!("{} Failed to apply manifest.", "✘".red());
    }
    Ok(())
}

async fn run_top() -> Result<()> {
    let mut nucleus = Nucleus::Client::connect().await.context("Nucleus unreachable")?;
    print!("\x1B[2J\x1B[1;1H");
    loop {
        let status = nucleus.status().await?;
        print!("\x1B[2J\x1B[1;1H");
        println!("CELL TOP - Lattice Status (Uptime: {}s)", status.uptime_secs);
        println!("-------------------------");
        println!("{:<20} | {:<10}", "Cell", "Status");
        println!("-------------------------");
        for cell in status.managed_cells {
            println!("{:<20} | Online", cell);
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn tail_logs(target: String) -> Result<()> {
    let mut observer = Observer::Client::connect().await.context("Observer unreachable")?;
    println!("{} Tailing logs for {}...", "→".blue(), target);
    loop {
        let logs = observer.tail(10).await?;
        for entry in logs {
            if entry.span.service.contains(&target) {
                println!("[{}] {} ({}us)", entry.span.trace_id, entry.span.name, entry.span.duration_us);
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn prune_cells() -> Result<()> {
    let mut nucleus = Nucleus::Client::connect().await.context("Nucleus unreachable")?;
    println!("{} Calculating unused cells (garbage collection)...", "→".blue());
    
    let result = nucleus.vacuum().await?;
    
    if result.killed.is_empty() {
        println!("{} No unused cells found.", "ℹ".blue());
    } else {
        println!("{} Pruned {} cells:", "✔".green(), result.killed.len());
        for cell in result.killed {
            println!("   - {}", cell.red());
        }
    }
    Ok(())
}

async fn send_request<T: cell_model::rkyv::Serialize<cell_model::rkyv::ser::serializers::AllocSerializer<256>>>(
    stream: &mut UnixStream,
    req: &T,
) -> Result<()> {
    let bytes = cell_model::rkyv::to_bytes::<_, 256>(req)?.into_vec();
    stream.write_all(&(bytes.len() as u32).to_le_bytes()).await?;
    stream.write_all(&bytes).await?;
    Ok(())
}

async fn recv_response<T: cell_model::rkyv::Archive>(stream: &mut UnixStream) -> Result<T>
where
    T::Archived: cell_model::rkyv::Deserialize<T, cell_model::rkyv::Infallible>
        + for<'a> cell_model::rkyv::CheckBytes<cell_model::rkyv::validation::validators::DefaultValidator<'a>>,
{
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;

    let archived = cell_model::rkyv::check_archived_root::<T>(&buf)
        .map_err(|e| anyhow::anyhow!("Protocol error: {:?}", e))?;
    Ok(archived.deserialize(&mut cell_model::rkyv::Infallible).unwrap())
}