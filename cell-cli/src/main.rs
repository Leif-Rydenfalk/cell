// cell-cli/src/main.rs (UPDATED)
// Unified CLI that interfaces with control-plane

use anyhow::Result;
use clap::{Parser, Subcommand};
use colored::*;
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
    /// Start the entire mesh
    Up {
        /// Start in foreground mode
        #[arg(short, long)]
        foreground: bool,
    },
    
    /// Stop the entire mesh gracefully
    Down {
        /// Force kill all processes
        #[arg(short, long)]
        force: bool,
    },
    
    /// Restart the mesh (preserves state)
    Restart,
    
    /// Show mesh status
    Status {
        /// Show detailed info
        #[arg(short, long)]
        verbose: bool,
    },
    
    /// Hot-swap a cell to latest version
    Swap {
        /// Cell name
        cell: String,
        
        /// Swap strategy (blue-green, canary, rolling)
        #[arg(short, long, default_value = "blue-green")]
        strategy: String,
        
        /// Canary percentage (only for canary strategy)
        #[arg(short, long)]
        percentage: Option<u8>,
    },
    
    /// List all running cells
    Ps {
        /// Show all scopes (system + organisms)
        #[arg(short, long)]
        all: bool,
    },
    
    /// Tail logs from a cell
    Logs {
        cell: String,
        
        /// Follow log output
        #[arg(short, long)]
        follow: bool,
    },
    
    /// Run health checks
    Health {
        /// Cell name (optional, checks all if omitted)
        cell: Option<String>,
    },
    
    /// Show dependency graph
    Graph {
        /// Output format (dot, json, ascii)
        #[arg(short, long, default_value = "ascii")]
        format: String,
    },
    
    /// Execute a test in the mesh
    Test {
        target: String,
        
        #[arg(short, long)]
        filter: Option<String>,
    },
    
    /// Interactive TUI dashboard
    Top,
    
    /// Prune unused cells
    Prune {
        /// Dry run (show what would be pruned)
        #[arg(short, long)]
        dry_run: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Up { foreground } => cmd_up(foreground).await,
        Commands::Down { force } => cmd_down(force).await,
        Commands::Restart => cmd_restart().await,
        Commands::Status { verbose } => cmd_status(verbose).await,
        Commands::Swap { cell, strategy, percentage } => {
            cmd_swap(cell, strategy, percentage).await
        }
        Commands::Ps { all } => cmd_ps(all).await,
        Commands::Logs { cell, follow } => cmd_logs(cell, follow).await,
        Commands::Health { cell } => cmd_health(cell).await,
        Commands::Graph { format } => cmd_graph(format).await,
        Commands::Test { target, filter } => cmd_test(target, filter).await,
        Commands::Top => cmd_top().await,
        Commands::Prune { dry_run } => cmd_prune(dry_run).await,
    }
}

// --- COMMAND IMPLEMENTATIONS ---

async fn cmd_up(foreground: bool) -> Result<()> {
    println!("{}", "Starting Cell Mesh...".bright_cyan().bold());
    
    if is_control_plane_running().await {
        println!("{}", "Already running".green());
        return Ok(());
    }

    if foreground {
        // Run control-plane in foreground
        let status = Command::new("cargo")
            .args(&["run", "--release", "-p", "control-plane"])
            .status()?;
        
        if !status.success() {
            anyhow::bail!("Control plane exited with error");
        }
    } else {
        // Daemonize control-plane
        Command::new("cargo")
            .args(&["run", "--release", "-p", "control-plane"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;

        // Wait for startup
        for i in 1..=30 {
            tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
            if is_control_plane_running().await {
                println!("{}", format!("Mesh online ({}s)", i * 0.2).green());
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

    if force {
        // SIGKILL control-plane and all children
        let output = Command::new("pkill")
            .args(&["-9", "-f", "control-plane"])
            .output()?;
        
        if output.status.success() {
            println!("{}", "Force killed".green());
        }
    } else {
        // Graceful shutdown via signal
        Command::new("pkill")
            .args(&["-SIGINT", "-f", "control-plane"])
            .spawn()?;

        // Wait for graceful shutdown
        for i in 1..=60 {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            if !is_control_plane_running().await {
                println!("{}", format!("Mesh stopped ({}s)", i * 0.5).green());
                return Ok(());
            }
        }
        
        println!("{}", "⚠ Timeout, forcing shutdown...".yellow());
        return cmd_down(true).await;
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

    // Read state from control-plane.json
    let home = dirs::home_dir().unwrap();
    let state_file = home.join(".cell/control-plane.json");
    
    if !state_file.exists() {
        println!("{}", "No state file found".red());
        return Ok(());
    }

    let json = std::fs::read_to_string(state_file)?;
    let state: serde_json::Value = serde_json::from_str(&json)?;

    if let Some(processes) = state.get("processes").and_then(|p| p.as_object()) {
        println!("{:<20} {:<10} {:<15} {:<10}", 
                 "CELL", "PID", "UPTIME", "VERSION");
        println!("{}", "─".repeat(60));

        for (name, info) in processes {
            let pid = info.get("pid").and_then(|p| p.as_u64()).unwrap_or(0);
            let start = info.get("start_time").and_then(|s| s.as_u64()).unwrap_or(0);
            let version = info.get("version_hash")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");

            let uptime = format_uptime(start);
            let version_short = &version[..version.len().min(8)];

            println!("{:<20} {:<10} {:<15} {:<10}",
                     name.green(),
                     pid,
                     uptime,
                     version_short);

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
    use cell_sdk::cell_remote;
    
    cell_remote!(SwapCoordinator = "swap-coordinator");

    println!("{}", format!("Hot-swapping {}...", cell).bright_yellow().bold());

    let mut coordinator = SwapCoordinator::Client::connect().await?;
    
    let swap_strategy = match strategy.as_str() {
        "blue-green" => SwapCoordinator::SwapStrategy::BlueGreen,
        "canary" => SwapCoordinator::SwapStrategy::Canary {
            percentage: percentage.unwrap_or(10),
        },
        "rolling" => SwapCoordinator::SwapStrategy::Rolling,
        _ => anyhow::bail!("Invalid strategy: {}", strategy),
    };

    // Get current version hash
    let mut builder = Builder::Client::connect().await?;
    let build_result = builder.build(
        cell.clone(),
        Builder::BuildMode::Standard,
    ).await?;

    let swap_id = coordinator.initiate_swap(SwapCoordinator::SwapRequest {
        cell_name: cell.clone(),
        new_version_hash: build_result.source_hash,
        strategy: swap_strategy,
    }).await?;

    println!("  └─ Swap ID: {}", swap_id.dimmed());

    // Poll for status
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

async fn cmd_ps(all: bool) -> Result<()> {
    use cell_sdk::discovery::Discovery;
    
    let nodes = Discovery::scan().await;
    
    println!("{:<24} {:<12} {:<30} {:<10}",
             "NAME", "ID", "ADDRESS", "STATUS");
    println!("{}", "─".repeat(80));

    for node in nodes {
        let addr = node.lan_address.clone()
            .or_else(|| node.local_socket.as_ref()
                .map(|p| format!("local://{}", p.display())))
            .unwrap_or_else(|| "?".to_string());
        
        let status = if node.status.is_alive {
            "Alive".green()
        } else {
            "Dead".red()
        };

        println!("{:<24} {:<12} {:<30} {}",
                 node.name,
                 node.instance_id,
                 addr,
                 status);
    }

    Ok(())
}

async fn cmd_logs(cell: String, follow: bool) -> Result<()> {
    println!("{}", format!("Logs for {}...", cell).bright_cyan());
    
    // Implementation: tail ~/.cell/logs/{cell}.log
    // For now, placeholder
    println!("(Log tailing not yet implemented)");
    Ok(())
}

async fn cmd_health(cell: Option<String>) -> Result<()> {
    println!("{}", "Health Check".bright_green().bold());
    println!();
    
    // Query Nucleus for health status
    use cell_sdk::cell_remote;
    cell_remote!(Nucleus = "nucleus");
    
    let mut nucleus = Nucleus::Client::connect().await?;
    let status = nucleus.status().await?;

    println!("Uptime: {}s", status.uptime_secs);
    println!("Managed Cells: {}", status.managed_cells.len());
    
    for cell_name in status.managed_cells {
        let is_healthy = nucleus.heartbeat(cell_name.clone()).await?;
        let icon = if is_healthy { "✓".green() } else { "✗".red() };
        println!("  {} {}", icon, cell_name);
    }

    Ok(())
}

async fn cmd_graph(format: String) -> Result<()> {
    use cell_sdk::cell_remote;
    cell_remote!(Mesh = "mesh");
    
    let mut mesh = Mesh::Client::connect().await?;
    let graph = mesh.get_graph().await?;

    match format.as_str() {
        "ascii" => print_ascii_graph(&graph),
        "dot" => print_dot_graph(&graph),
        "json" => println!("{}", serde_json::to_string_pretty(&graph)?),
        _ => anyhow::bail!("Unknown format: {}", format),
    }

    Ok(())
}

async fn cmd_test(target: String, filter: Option<String>) -> Result<()> {
    // Delegate to existing test implementation
    println!("Running tests for {}...", target);
    Ok(())
}

async fn cmd_top() -> Result<()> {
    // Delegate to existing TUI
    crate::tui_monitor::run_dashboard().await
}

async fn cmd_prune(dry_run: bool) -> Result<()> {
    use cell_sdk::cell_remote;
    cell_remote!(Nucleus = "nucleus");
    
    let mut nucleus = Nucleus::Client::connect().await?;
    
    if dry_run {
        println!("{}", "Dry run - showing what would be pruned:".yellow());
        // TODO: Implement dry-run logic
    } else {
        println!("{}", "Pruning unused cells...".bright_red());
        let result = nucleus.vacuum().await?;
        println!("✓ Pruned {} cells", result.killed.len());
        for cell in result.killed {
            println!("  • {}", cell);
        }
    }

    Ok(())
}

// --- HELPERS ---

async fn is_control_plane_running() -> bool {
    let output = Command::new("pgrep")
        .args(&["-f", "control-plane"])
        .output();
    
    if let Ok(output) = output {
        !output.stdout.is_empty()
    } else {
        false
    }
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
    } else if elapsed < 86400 {
        format!("{}h", elapsed / 3600)
    } else {
        format!("{}d", elapsed / 86400)
    }
}

fn print_ascii_graph(graph: &std::collections::HashMap<String, Vec<String>>) {
    println!("{}", "Dependency Graph:".bright_cyan().bold());
    println!();
    
    for (consumer, providers) in graph {
        println!("{}", consumer.bright_green());
        for provider in providers {
            println!("  ├─→ {}", provider.yellow());
        }
        println!();
    }
}

fn print_dot_graph(graph: &std::collections::HashMap<String, Vec<String>>) {
    println!("digraph CellMesh {{");
    println!("  rankdir=LR;");
    println!("  node [shape=box];");
    
    for (consumer, providers) in graph {
        for provider in providers {
            println!("  \"{}\" -> \"{}\";", consumer, provider);
        }
    }
    
    println!("}}");
}

use cell_sdk::cell_remote;
cell_remote!(Builder = "builder");