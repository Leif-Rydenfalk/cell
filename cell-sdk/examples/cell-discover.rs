// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

//! CLI tool to discover and list all Cells (LAN + Local)

use anyhow::Result;
use cell_sdk::{discovery::Discovery, pheromones::PheromoneSystem};
use clap::Parser;

#[derive(Parser)]
#[command(name = "cell-discover")]
#[command(about = "Discover Cells on LAN and Localhost", long_about = None)]
struct Cli {
    /// Wait time in seconds for LAN packets (default: 1)
    #[arg(short, long, default_value_t = 1)]
    wait: u64,

    /// Watch mode - continuously update the list
    #[arg(short = 'w', long)]
    watch: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Start passive discovery (listens for LAN broadcasts)
    let _pheromones = PheromoneSystem::ignite().await?;

    if cli.watch {
        println!("Watch mode - Press Ctrl+C to exit\n");
        loop {
            // In watch mode, we clear screen and refresh
            print!("\x1B[2J\x1B[1;1H");
            println!("🔍 Cell Discovery (Live)\n");
            display_cells().await;
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    } else {
        // One-shot mode: Wait briefly to populate LAN cache
        if cli.wait > 0 {
            // println!("Listening for network broadcasts for {}s...", cli.wait);
            tokio::time::sleep(std::time::Duration::from_secs(cli.wait)).await;
        }
        display_cells().await;
    }

    Ok(())
}

async fn display_cells() {
    let nodes = Discovery::scan().await;

    if nodes.is_empty() {
        println!("No Cells found (neither on LAN nor Local Sockets).");
        return;
    }

    println!("┌─────────────────────┬──────────────────────┬──────────────────────┐");
    println!("│ Cell Name           │ Network (LAN)        │ Local Socket         │");
    println!("├─────────────────────┼──────────────────────┼──────────────────────┤");

    for node in nodes {
        let lan_str = node.lan_address.unwrap_or_else(|| "---".into());

        let sock_str = if let Some(path) = node.local_socket {
            // Show only filename for brevity if in default dir, or full path?
            // Let's show "Yes" or "Active" or just the path stem if overly long
            if path.to_string_lossy().len() > 20 {
                "Present".to_string()
            } else {
                path.file_name().unwrap().to_string_lossy().to_string()
            }
        } else {
            "---".into()
        };

        println!(
            "│ {:<19} │ {:<20} │ {:<20} │",
            truncate(&node.name, 19),
            truncate(&lan_str, 20),
            sock_str
        );
    }

    println!("└─────────────────────┴──────────────────────┴──────────────────────┘");
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() > max_len {
        format!("{}...", &s[..max_len - 3])
    } else {
        s.to_string()
    }
}
