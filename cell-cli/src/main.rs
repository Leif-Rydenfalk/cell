// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

mod tui_monitor;

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
use cell_sdk::discovery::Discovery;
use serde::Deserialize as SerdeDeserialize;
use std::collections::{HashMap, HashSet};
use cell_sdk::channel;
// Added for bootstrapping
use std::process::{Command, Stdio};

// === SYSTEM CELLS ===
cell_remote!(Nucleus = "nucleus");
cell_remote!(Observer = "observer");
cell_remote!(Builder = "builder");
cell_remote!(Mesh = "mesh");

#[derive(Parser)]
#[command(name = "cell")]
#[command(about = "Cell Substrate Control Interface", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Build and run the current cell in release mode, then monitor it
    Run {
        /// Release mode
        #[arg(long, default_value_t = true)]
        release: bool,
    },
    /// Spawn a cell by name from the registry
    Spawn { name: String },
    /// List all running cells (System & LAN)
    Ps,
    /// Kill a cell and optionally its dependencies
    Kill {
        target: String,
        /// Also kill cells that depend on this one
        #[arg(short, long)]
        cascade: bool,
    },
    /// Kill all cell processes found on the system (cleanup)
    Cleanup,
    /// Run tests
    Test { target: String, #[arg(short, long)] filter: Option<String> },
    /// Apply workspace or manifest
    Up { #[arg(short, long)] file: Option<String> },
    /// Live TUI Dashboard
    Top,
    /// Follow logs
    Logs { target: String },
    /// GC unused cells
    Prune,
}

#[derive(SerdeDeserialize)]
struct CellWorkspace { workspace: WorkspaceConfig }
#[derive(SerdeDeserialize)]
struct WorkspaceConfig { members: Vec<String> }

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run { release } => run_dev_cell(release).await,
        Commands::Spawn { name } => spawn_cell(name).await,
        Commands::Ps => run_ps().await,
        Commands::Kill { target, cascade } => run_kill(target, cascade).await,
        Commands::Cleanup => run_cleanup().await,
        Commands::Test { target, filter } => run_test(target, filter).await,
        Commands::Up { file } => run_up(file).await,
        Commands::Top => tui_monitor::run_dashboard().await,
        Commands::Logs { target } => tail_logs(target).await,
        Commands::Prune => prune_cells().await,
    }
}

async fn run_dev_cell(release: bool) -> Result<()> {
    let manifest_path = PathBuf::from("Cargo.toml");
    if !manifest_path.exists() {
        return Err(anyhow!("No Cargo.toml found. Run this from a cell directory."));
    }
    
    let content = fs::read_to_string(&manifest_path)?;
    let parsed: toml::Value = toml::from_str(&content)?;
    let package_name = parsed.get("package")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        .ok_or_else(|| anyhow!("Invalid Cargo.toml"))?;

    println!("{} Building {} (release={})...", "⚙".blue(), package_name, release);

    let mut builder = Builder::Client::connect().await
        .context("Builder service unreachable. Is 'cell up' running?")?;

    let req = Builder::BuildRequest {
        cell_name: package_name.to_string(),
        mode: Builder::BuildMode::Standard,
    };

    let res = builder.build(req).await.context("Build failed")?;

    println!("{} Build complete. Hash: {:.8}", "✔".green(), res.source_hash);

    spawn_cell(package_name.to_string()).await?;

    println!("{} Attaching monitor...", "→".blue());
    tokio::time::sleep(Duration::from_secs(1)).await;
    tui_monitor::run_dashboard().await
}

async fn run_ps() -> Result<()> {
    let nodes = Discovery::scan().await;
    println!("{:<20} {:<10} {:<25} {:<10}", "NAME", "ID", "ADDRESS", "STATUS");
    println!("{}", "-".repeat(65));
    
    for node in nodes {
        let addr = node.lan_address.clone()
            .or_else(|| node.local_socket.as_ref().map(|p| "local://".to_string() + p.to_string_lossy().as_ref()))
            .unwrap_or("?".to_string());
            
        let status = if node.status.is_alive { "Alive".green() } else { "Dead".red() };
        
        println!("{:<20} {:<10} {:<25} {}", node.name, node.instance_id, addr, status);
    }
    Ok(())
}

async fn run_kill(target: String, cascade: bool) -> Result<()> {
    let mut kill_list = vec![target.clone()];

    if cascade {
        println!("{} Analyzing dependencies for cascade kill...", "ℹ".blue());
        let mut mesh = Mesh::Client::connect().await.context("Mesh service unavailable")?;
        
        let graph = mesh.get_graph().await?;
        
        let mut reverse_graph: HashMap<String, Vec<String>> = HashMap::new();
        for (consumer, providers) in graph {
            for provider in providers {
                reverse_graph.entry(provider).or_default().push(consumer.clone());
            }
        }

        let mut queue = vec![target.clone()];
        let mut visited = HashSet::new();
        visited.insert(target.clone());

        while let Some(current) = queue.pop() {
            if let Some(dependents) = reverse_graph.get(&current) {
                for dep in dependents {
                    if !visited.contains(dep) {
                        visited.insert(dep.clone());
                        kill_list.push(dep.clone());
                        queue.push(dep.clone());
                    }
                }
            }
        }
    }

    println!("{} Stopping {} cells...", "☢".red(), kill_list.len());
    
    for cell in kill_list {
        print!("   Killing {}... ", cell);
        use std::io::Write;
        std::io::stdout().flush()?;

        if let Ok(mut synapse) = cell_sdk::Synapse::grow(&cell).await {
            let req = cell_model::ops::OpsRequest::Shutdown;
            let req_bytes = cell_model::rkyv::to_bytes::<_, 256>(&req)?.into_vec();
            let _ = synapse.fire_on_channel(channel::OPS, &req_bytes).await;
            println!("{}", "Sent".yellow());
        } else {
            println!("{}", "Unreachable".red());
        }
    }
    
    Ok(())
}

async fn run_cleanup() -> Result<()> {
    println!("{} Scanning system for stray Cell processes...", "🔍".blue());
    
    let mut system = sysinfo::System::new_all();
    system.refresh_all();
    
    let my_pid = sysinfo::get_current_pid().unwrap();
    let mut killed_count = 0;

    for (pid, process) in system.processes() {
        if *pid == my_pid { continue; }

        let name = process.name().to_string();
        
        let cmd = process.cmd();
        if cmd.is_empty() { continue; }
        
        let is_kernel = matches!(name.as_ref(), "mycelium" | "hypervisor" | "nucleus" | "axon" | "builder");
        
        let exe_path = process.exe().unwrap_or(Path::new(""));
        let is_cell_bin = exe_path.to_string_lossy().contains("/.cell/") || 
                          exe_path.to_string_lossy().contains("/target/");

        if is_kernel || is_cell_bin {
            println!("   Found {} ({}) - Terminating...", name, pid);
            if process.kill() {
                killed_count += 1;
            } else {
                println!("   {} Failed to kill {}", "⚠".yellow(), pid);
            }
        }
    }

    if killed_count > 0 {
        println!("{} Cleaned up {} stray processes.", "✔".green(), killed_count);
    } else {
        println!("{} No stray cells found.", "ℹ".blue());
    }
    Ok(())
}

// --- BOOTSTRAPPING LOGIC ---

async fn get_socket_path() -> PathBuf {
    let home = dirs::home_dir().expect("No HOME");
    if let Ok(p) = std::env::var("CELL_SOCKET_DIR") {
        PathBuf::from(p).join("mitosis.sock")
    } else {
        home.join(".cell/runtime/system/mitosis.sock")
    }
}

async fn bootstrap_system() -> Result<()> {
    println!("{} Bootstrapping Cell System Daemon...", "⚡".yellow());
    
    // Look for mycelium binary
    let bins = ["mycelium", "target/release/mycelium", "target/debug/mycelium"];
    let mut mycelium_bin = None;
    
    for bin in bins {
        if let Ok(path) = fs::canonicalize(bin) {
            mycelium_bin = Some(path);
            break;
        }
    }
    
    // Fallback: Check Cargo path
    if mycelium_bin.is_none() {
        if let Ok(path) = which::which("mycelium") {
            mycelium_bin = Some(path);
        }
    }

    let status = if let Some(bin) = mycelium_bin {
        println!("   Using binary: {:?}", bin);
        Command::new(bin)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    } else {
        // Last resort: cargo run
        println!("   Using 'cargo run'...");
        Command::new("cargo")
            .args(&["run", "--release", "-p", "mycelium"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    };

    if let Err(e) = status {
        return Err(anyhow!("Failed to start mycelium: {}", e));
    }

    // Wait for socket
    let socket = get_socket_path().await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    
    print!("   Waiting for Hypervisor socket...");
    use std::io::Write;
    
    while tokio::time::Instant::now() < deadline {
        if socket.exists() {
            if UnixStream::connect(&socket).await.is_ok() {
                println!("{}", " OK".green());
                return Ok(());
            }
        }
        print!(".");
        std::io::stdout().flush()?;
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    println!("{}", " FAILED".red());
    Err(anyhow!("Timed out waiting for system boot."))
}

async fn connect_daemon() -> Result<UnixStream> {
    let socket_path = get_socket_path().await;

    // Try connect
    match UnixStream::connect(&socket_path).await {
        Ok(s) => Ok(s),
        Err(_) => {
            // Auto-bootstrap
            bootstrap_system().await?;
            // Retry
            UnixStream::connect(&socket_path).await.context("Daemon unreachable after bootstrap")
        }
    }
}

async fn spawn_cell(name: String) -> Result<()> {
    let mut stream = connect_daemon().await?;
    let req = MitosisRequest::Spawn { cell_name: name.clone(), config: None };
    send_request(&mut stream, &req).await?;
    let resp: MitosisResponse = recv_response(&mut stream).await?;
    match resp {
        MitosisResponse::Ok { socket_path } => println!("{} Spawned {} at {}", "✔".green(), name, socket_path),
        MitosisResponse::Denied { reason } => return Err(anyhow!("Spawn denied: {}", reason)),
    }
    Ok(())
}

async fn run_up(file: Option<String>) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let toml_path = cwd.join("Cell.toml");
    if file.is_none() && toml_path.exists() { return run_workspace_up(&toml_path).await; }
    let target = file.ok_or_else(|| anyhow!("No Cell.toml or file specified"))?;
    if target.ends_with(".toml") || target == "Cell.toml" { run_workspace_up(Path::new(&target)).await } 
    else { apply_manifest(target).await }
}

async fn run_workspace_up(path: &Path) -> Result<()> {
    let content = fs::read_to_string(path)?;
    let config: CellWorkspace = toml::from_str(&content)?;
    let root_dir = path.parent().unwrap();
    let registry_dir = dirs::home_dir().expect("No HOME").join(".cell/registry");
    fs::create_dir_all(&registry_dir)?;

    println!("{} Found {} members", "ℹ".blue(), config.workspace.members.len());
    for member in &config.workspace.members {
        let member_path = root_dir.join(member);
        if !member_path.exists() { continue; }
        let link_path = registry_dir.join(member);
        if link_path.exists() { let _ = fs::remove_file(&link_path); }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&member_path, &link_path)?;
    }

    for cell in &config.workspace.members {
        if let Err(e) = spawn_cell(cell.clone()).await { println!("Error spawning {}: {}", cell, e); }
    }
    println!("\n{} Workspace active.", "✔".green());
    Ok(())
}

async fn run_test(target: String, filter: Option<String>) -> Result<()> {
    println!("{} Connecting...", "Cell".blue().bold());
    let mut stream = connect_daemon().await?;
    send_request(&mut stream, &MitosisRequest::Test { target_cell: target, filter }).await?;
    loop {
        let mut len_buf = [0u8; 4];
        if stream.read_exact(&mut len_buf).await.is_err() { break; }
        let len = u32::from_le_bytes(len_buf) as usize;
        let mut buf = vec![0u8; len];
        stream.read_exact(&mut buf).await?;
        let event: TestEvent = cell_model::rkyv::check_archived_root::<TestEvent>(&buf).unwrap().deserialize(&mut cell_model::rkyv::Infallible).unwrap();
        match event {
            TestEvent::Log(msg) => println!("  {}", msg.dimmed()),
            TestEvent::CaseFinished { name, success, .. } => println!(" {} {}", if success { "✔".green() } else { "✘".red() }, name),
            TestEvent::SuiteFinished { failed, .. } => if failed > 0 { std::process::exit(1); } else { break; },
            _ => {}
        }
    }
    Ok(())
}

async fn apply_manifest(path: String) -> Result<()> {
    let yaml = fs::read_to_string(&path)?;
    let mut nucleus = Nucleus::Client::connect().await?;
    if nucleus.apply(Nucleus::ApplyManifest { yaml }).await? { println!("{} Applied.", "✔".green()); }
    Ok(())
}

async fn tail_logs(target: String) -> Result<()> {
    let mut observer = Observer::Client::connect().await?;
    println!("{} Tailing logs for {}...", "→".blue(), target);
    loop {
        for entry in observer.tail(10).await? {
            if entry.span.service.contains(&target) {
                println!("[{}] {}", entry.span.trace_id, entry.span.name);
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn prune_cells() -> Result<()> {
    let mut nucleus = Nucleus::Client::connect().await?;
    let result = nucleus.vacuum().await?;
    println!("{} Pruned {} cells", "✔".green(), result.killed.len());
    Ok(())
}

async fn send_request<T: cell_model::rkyv::Serialize<cell_model::rkyv::ser::serializers::AllocSerializer<256>>>(stream: &mut UnixStream, req: &T) -> Result<()> {
    let bytes = cell_model::rkyv::to_bytes::<_, 256>(req)?.into_vec();
    stream.write_all(&(bytes.len() as u32).to_le_bytes()).await?;
    stream.write_all(&bytes).await?;
    Ok(())
}

async fn recv_response<T: cell_model::rkyv::Archive>(stream: &mut UnixStream) -> Result<T> 
where T::Archived: cell_model::rkyv::Deserialize<T, cell_model::rkyv::Infallible> + for<'a> cell_model::rkyv::CheckBytes<cell_model::rkyv::validation::validators::DefaultValidator<'a>>
{
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;
    Ok(cell_model::rkyv::check_archived_root::<T>(&buf).map_err(|e| anyhow!("Proto: {:?}", e))?.deserialize(&mut cell_model::rkyv::Infallible).unwrap())
}