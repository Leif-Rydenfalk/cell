// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::Result;
use cell_sdk::{discovery::Discovery, pheromones::PheromoneSystem};
use clap::Parser;
use std::collections::HashSet;
use std::time::Duration;
use tokio::time::interval;

#[derive(Parser)]
#[command(name = "cell-discover")]
struct Cli {
    /// Skip latency probing
    #[arg(short = 'n', long)]
    no_probe: bool,

    /// Scan interval in ms
    #[arg(short, long, default_value_t = 500)]
    interval: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Start background listener
    let _pheromones = PheromoneSystem::ignite().await?;

    println!("Scanning network for cells... (Ctrl+C to quit)");

    let mut seen = HashSet::new();
    let mut ticker = interval(Duration::from_millis(cli.interval));

    loop {
        ticker.tick().await;

        let nodes = Discovery::scan().await;

        for mut node in nodes {
            if seen.contains(&node.name) {
                continue;
            }
            seen.insert(node.name.clone());

            let do_probe = !cli.no_probe;

            tokio::spawn(async move {
                if do_probe {
                    node.probe().await;
                }

                let (addr, lat) = get_details(&node);
                println!("{:<24} {:<24} {}", node.name, addr, lat);
            });
        }
    }
}

fn get_details(node: &cell_sdk::discovery::CellNode) -> (String, String) {
    let addr = if let Some(a) = &node.lan_address {
        a.clone()
    } else if node.local_socket.is_some() {
        "local".to_string()
    } else {
        "unknown".to_string()
    };

    let lat = if let Some(d) = node.status.local_latency.or(node.status.lan_latency) {
        let micros = d.as_micros();
        if micros < 1000 {
            format!("{}µs", micros)
        } else {
            format!("{:.1}ms", d.as_secs_f64() * 1000.0)
        }
    } else {
        "-".to_string()
    };

    (addr, lat)
}
