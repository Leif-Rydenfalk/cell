// cell-cli/src/main.rs
// Unified CLI that interfaces with control-plane

mod tui_monitor;

use anyhow::{Context, Result};
use cell_sdk::cell_remote;
use clap::{Parser, Subcommand};
use colored::*;
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[derive(Parser)]
#[command(name = "cell")]
#[command(about = "Cell Substrate Control Interface", version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Up {
        #[arg(short, long)]
        foreground: bool,
    },
    Down {
        #[arg(short, long)]
        force: bool,
    },
    Restart,
    Status {
        #[arg(short, long)]
        verbose: bool,
    },
    Swap {
        cell: String,
        #[arg(short, long, default_value = "blue-green")]
        strategy: String,
        #[arg(short, long)]
        percentage: Option<u8>,
    },
    Ps {
        #[arg(short, long)]
        all: bool,
    },
    Logs {
        cell: String,
        #[arg(short, long)]
        follow: bool,
    },
    Health {
        cell: Option<String>,
    },
    Graph {
        #[arg(short, long, default_value = "ascii")]
        format: String,
    },
    Test {
        target: String,
        #[arg(short, long)]
        filter: Option<String>,
    },
    Top,
    Prune {
        #[arg(short, long)]
        dry_run: bool,
    },
    Clone {
        url: String,
        #[arg(short, long)]
        name: Option<String>,
    },
    Register {
        path: PathBuf,
        #[arg(short, long)]
        name: Option<String>,
    },
    List,
    Run {
        #[arg(long)]
        release: bool,
    },
}

// Define remotes at module level to avoid duplication/scope issues
cell_remote!(Builder = "builder");
cell_remote!(SwapCoordinator = "swap-coordinator");

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Up { foreground } => cmd_up(foreground).await,
        Commands::Down { force } => cmd_down(force).await,
        Commands::Restart => cmd_restart().await,
        Commands::Status { verbose } => cmd_status(verbose).await,
        Commands::Swap {
            cell,
            strategy,
            percentage,
        } => cmd_swap(cell, strategy, percentage).await,
        Commands::Ps { all } => cmd_ps(all).await,
        Commands::Logs { cell, follow } => cmd_logs(cell, follow).await,
        Commands::Health { cell } => cmd_health(cell).await,
        Commands::Graph { format } => cmd_graph(format).await,
        Commands::Test { target, filter } => cmd_test(target, filter).await,
        Commands::Top => cmd_top().await,
        Commands::Prune { dry_run } => cmd_prune(dry_run).await,
        Commands::Clone { url, name } => cmd_clone(url, name).await,
        Commands::Register { path, name } => cmd_register(path, name).await,
        Commands::List => cmd_list().await,
        Commands::Run { release } => cmd_run(release).await,
    }
}

async fn cmd_up(foreground: bool) -> Result<()> {
    println!("{}", "Starting Cell Mesh...".bright_cyan().bold());
    if is_control_plane_running().await {
        println!("{}", "Already running".green());
        return Ok(());
    }
    if foreground {
        let status = Command::new("cargo")
            .args(&["run", "--release", "-p", "control-plane"])
            .status()?;
        if !status.success() {
            anyhow::bail!("Control plane exited with error");
        }
    } else {
        Command::new("cargo")
            .args(&["run", "--release", "-p", "control-plane"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        for i in 1..=30 {
            tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
            if is_control_plane_running().await {
                println!(
                    "{}",
                    format!("Mesh online ({:.1}s)", i as f64 * 0.2).green()
                );
                return Ok(());
            }
        }
        anyhow::bail!("Timeout waiting for control plane");
    }
    Ok(())
}

async fn cmd_down(force: bool) -> Result<()> {
    println!("{}", "Stopping Cell Mesh...".bright_red().bold());
    if !is_control_plane_running().await {
        println!("{}", "Already stopped".yellow());
        return Ok(());
    }

    if !force {
        // Graceful attempt
        Command::new("pkill")
            .args(&["-SIGINT", "-f", "control-plane"])
            .spawn()?;
        for i in 1..=60 {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            if !is_control_plane_running().await {
                println!(
                    "{}",
                    format!("Mesh stopped ({:.1}s)", i as f64 * 0.5).green()
                );
                return Ok(());
            }
        }
        println!("{}", "⚠ Timeout, forcing shutdown...".yellow());
    }

    // Force kill (either explicitly requested or fallback)
    let output = Command::new("pkill")
        .args(&["-9", "-f", "control-plane"])
        .output()?;
    if output.status.success() {
        println!("{}", "Force killed".green());
    }
    Ok(())
}

async fn cmd_restart() -> Result<()> {
    println!("{}", "Restarting Cell Mesh...".bright_yellow().bold());
    cmd_down(false).await?;
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
    cmd_up(false).await
}

async fn cmd_status(verbose: bool) -> Result<()> {
    if !is_control_plane_running().await {
        println!("{}", "Mesh is not running".red());
        return Ok(());
    }
    println!("{}", "Cell Mesh Status".bright_cyan().bold());
    println!();
    let home = dirs::home_dir().unwrap();
    let state_file = home.join(".cell/control-plane.json");
    if !state_file.exists() {
        println!("{}", "No state file found".red());
        return Ok(());
    }
    let json = std::fs::read_to_string(state_file)?;
    let state: serde_json::Value = serde_json::from_str(&json)?;
    if let Some(processes) = state.get("processes").and_then(|p| p.as_object()) {
        println!(
            "{:<20} {:<10} {:<15} {:<10}",
            "CELL", "PID", "UPTIME", "VERSION"
        );
        println!("{}", "─".repeat(60));
        for (name, info) in processes {
            let pid = info.get("pid").and_then(|p| p.as_u64()).unwrap_or(0);
            let start = info.get("start_time").and_then(|s| s.as_u64()).unwrap_or(0);
            let version = info
                .get("version_hash")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let uptime = format_uptime(start);
            let version_short = &version[..version.len().min(8)];
            println!(
                "{:<20} {:<10} {:<15} {:<10}",
                name.green(),
                pid,
                uptime,
                version_short
            );
            if verbose {
                if let Some(socket) = info.get("socket_path").and_then(|s| s.as_str()) {
                    println!("  └─ Socket: {}", socket.dimmed());
                }
            }
        }
    }
    Ok(())
}

async fn cmd_swap(cell: String, strategy: String, percentage: Option<u8>) -> Result<()> {
    println!(
        "{}",
        format!("Hot-swapping {}...", cell).bright_yellow().bold()
    );
    let mut coordinator = SwapCoordinator::Client::connect().await?;

    let swap_strategy = match strategy.as_str() {
        "blue-green" => SwapCoordinator::SwapStrategy::BlueGreen,
        "canary" => SwapCoordinator::SwapStrategy::Canary {
            percentage: percentage.unwrap_or(10),
        },
        "rolling" => SwapCoordinator::SwapStrategy::Rolling,
        _ => anyhow::bail!("Invalid strategy: {}", strategy),
    };

    let mut builder = Builder::Client::connect().await?;
    // Fixed: Construct request struct explicitly
    let req = Builder::BuildRequest {
        cell_name: cell.clone(),
        mode: Builder::BuildMode::Standard,
    };
    let build_result = builder.build(req).await?;

    let swap_id = coordinator
        .initiate_swap(SwapCoordinator::SwapRequest {
            cell_name: cell.clone(),
            new_version_hash: build_result.source_hash,
            strategy: swap_strategy,
        })
        .await?;

    println!("  └─ Swap ID: {}", swap_id.dimmed());

    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        if let Some(status) = coordinator.get_status(swap_id.clone()).await? {
            let phase_str = format!("{:?}", status.phase);
            println!("  └─ {} ({}%)", phase_str, status.progress);
            match status.phase {
                SwapCoordinator::SwapPhase::Completed => {
                    println!("{}", "Swap completed".green());
                    break;
                }
                SwapCoordinator::SwapPhase::Failed { .. } => {
                    println!("{}", "Swap failed".red());
                    break;
                }
                _ => {}
            }
        }
    }
    Ok(())
}

async fn cmd_ps(_all: bool) -> Result<()> {
    use cell_sdk::discovery::Discovery;
    let nodes = Discovery::scan().await;
    println!(
        "{:<24} {:<12} {:<30} {:<10}",
        "NAME", "ID", "ADDRESS", "STATUS"
    );
    println!("{}", "─".repeat(80));
    for node in nodes {
        let addr = node
            .lan_address
            .clone()
            .or_else(|| {
                node.local_socket
                    .as_ref()
                    .map(|p| format!("local://{}", p.display()))
            })
            .unwrap_or_else(|| "?".to_string());
        let status = if node.status.is_alive {
            "Alive".green()
        } else {
            "Dead".red()
        };
        println!(
            "{:<24} {:<12} {:<30} {}",
            node.name, node.instance_id, addr, status
        );
    }
    Ok(())
}

async fn cmd_logs(cell: String, _follow: bool) -> Result<()> {
    println!("{}", format!("Logs for {}...", cell).bright_cyan());
    println!("(Log tailing not yet implemented)");
    Ok(())
}

async fn cmd_health(cell: Option<String>) -> Result<()> {
    println!("{}", "Health Check".bright_green().bold());
    use cell_sdk::discovery::Discovery;
    let nodes = Discovery::scan().await;
    for node in nodes {
        if let Some(c) = &cell {
            if c != &node.name {
                continue;
            }
        }
        let status = if node.status.is_alive {
            "✓".green()
        } else {
            "✗".red()
        };
        println!("  {} {}", status, node.name);
    }
    Ok(())
}

async fn cmd_graph(format: String) -> Result<()> {
    let home = dirs::home_dir().unwrap();
    let state_file = home.join(".cell/control-plane.json");

    if !state_file.exists() {
        println!("{}", "No mesh state found".red());
        return Ok(());
    }

    let json = std::fs::read_to_string(state_file)?;
    let state: serde_json::Value = serde_json::from_str(&json)?;

    // Convert JSON dependencies to HashMap<String, Vec<String>>
    let mut graph = std::collections::HashMap::new();
    if let Some(deps) = state.get("dependencies").and_then(|d| d.as_object()) {
        for (consumer, providers) in deps {
            if let Some(provider_array) = providers.as_array() {
                let provider_strings: Vec<String> = provider_array
                    .iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect();
                graph.insert(consumer.clone(), provider_strings);
            }
        }
    }

    match format.as_str() {
        "ascii" => print_ascii_graph(&graph),
        "dot" => print_dot_graph(&graph),
        "json" => println!("{}", serde_json::to_string_pretty(&graph)?),
        _ => anyhow::bail!("Unknown format: {}", format),
    }
    Ok(())
}

async fn cmd_test(target: String, _filter: Option<String>) -> Result<()> {
    println!("Running tests for {}...", target);
    Ok(())
}

async fn cmd_top() -> Result<()> {
    crate::tui_monitor::run_dashboard().await
}

async fn cmd_prune(_dry_run: bool) -> Result<()> {
    println!("{}", "Prune feature is currently unavailable.".red());
    Ok(())
}

async fn cmd_clone(url: String, name: Option<String>) -> Result<()> {
    let repo_name = name.unwrap_or_else(|| {
        url.split('/')
            .last()
            .unwrap()
            .trim_end_matches(".git")
            .to_string()
    });
    let home = dirs::home_dir().expect("No HOME");
    let sources = home.join(".cell/sources");
    std::fs::create_dir_all(&sources)?;
    let target_dir = sources.join(&repo_name);
    println!("{} {} from {}...", "Cloning".green().bold(), repo_name, url);
    if target_dir.exists() {
        println!(
            "{} Source directory already exists at {:?}",
            "Warning:".yellow(),
            target_dir
        );
    } else {
        let status = Command::new("git")
            .arg("clone")
            .arg(&url)
            .arg(&target_dir)
            .status()
            .context("Failed to run git clone")?;
        if !status.success() {
            anyhow::bail!("Git clone failed");
        }
    }
    cmd_register(target_dir, Some(repo_name)).await
}

async fn cmd_register(path: PathBuf, name: Option<String>) -> Result<()> {
    let abs_path = std::fs::canonicalize(&path).context(format!("Path not found: {:?}", path))?;
    if !abs_path.is_dir() {
        anyhow::bail!("Path must be a directory");
    }
    let cell_name =
        name.unwrap_or_else(|| abs_path.file_name().unwrap().to_string_lossy().to_string());
    let home = dirs::home_dir().expect("No HOME");
    let registry = home.join(".cell/registry");
    std::fs::create_dir_all(&registry)?;
    let link_path = registry.join(&cell_name);
    if link_path.exists() || link_path.is_symlink() {
        std::fs::remove_file(&link_path).ok();
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(&abs_path, &link_path)?;
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&abs_path, &link_path)?;
    println!(
        "{} Registered '{}' -> {:?}",
        "✓".green(),
        cell_name,
        abs_path
    );
    Ok(())
}

async fn cmd_list() -> Result<()> {
    let home = dirs::home_dir().expect("No HOME");
    let registry = home.join(".cell/registry");
    if !registry.exists() {
        println!("No cells registered.");
        return Ok(());
    }
    println!("{:<20} {}", "CELL", "LOCATION");
    println!("{}", "─".repeat(60));
    let mut entries = std::fs::read_dir(&registry)?
        .filter_map(|e| e.ok())
        .collect::<Vec<_>>();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        let target = std::fs::read_link(entry.path())
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "???".to_string());
        println!("{:<20} {}", name.green(), target.dimmed());
    }
    Ok(())
}

async fn cmd_run(release: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    if cwd.join("Cargo.toml").exists() {
        let mut cmd = Command::new("cargo");
        cmd.arg("run");
        if release {
            cmd.arg("--release");
        }
        cmd.stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        if !cmd.status()?.success() {
            anyhow::bail!("Cell crashed");
        }
        return Ok(());
    }
    anyhow::bail!("Unknown project type in {:?}.", cwd);
}

async fn is_control_plane_running() -> bool {
    Command::new("pgrep")
        .args(&["-f", "control-plane"])
        .output()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false)
}

fn format_uptime(start_secs: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let elapsed = now - start_secs;
    if elapsed < 60 {
        format!("{}s", elapsed)
    } else if elapsed < 3600 {
        format!("{}m", elapsed / 60)
    } else {
        format!("{}h", elapsed / 3600)
    }
}

fn print_ascii_graph(graph: &std::collections::HashMap<String, Vec<String>>) {
    println!("{}", "Dependency Graph:".bright_cyan().bold());
    for (consumer, providers) in graph {
        println!("{}", consumer.bright_green());
        for provider in providers {
            println!("  ├─→ {}", provider.yellow());
        }
    }
}

fn print_dot_graph(graph: &std::collections::HashMap<String, Vec<String>>) {
    println!("digraph CellMesh {{");
    for (consumer, providers) in graph {
        for provider in providers {
            println!("  \"{}\" -> \"{}\";", consumer, provider);
        }
    }
    println!("}}");
}
