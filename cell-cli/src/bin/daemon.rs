use anyhow::{anyhow, Result};
use cell_cli::config::Genome;
use cell_cli::golgi::{AxonTerminal, Golgi, Target};
use cell_cli::{nucleus, sys_log, vacuole};
use clap::Parser;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio::io::AsyncWriteExt;
use tokio::sync::broadcast;
use tokio::task::JoinSet;

#[derive(Parser)]
#[command(name = "cell-daemon")]
struct DaemonCli {
    /// Path to the cell directory (containing genome.toml)
    dir: PathBuf,

    /// Path to the compiled binary to run
    #[arg(long)]
    bin: PathBuf,

    /// Enable Donor Mode
    #[arg(long)]
    donor: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = DaemonCli::parse();
    run_cell_runtime(&args.dir, args.bin, args.donor).await
}

async fn run_cell_runtime(dir: &Path, bin_path: PathBuf, is_donor: bool) -> Result<()> {
    // Load Genome
    let genome_path = dir.join("genome.toml");
    let txt = std::fs::read_to_string(&genome_path).map_err(|_| anyhow!("Missing genome.toml"))?;
    let dna: Genome = toml::from_str(&txt)?;

    let run_dir = dir.join("run");
    if run_dir.exists() {
        std::fs::remove_dir_all(&run_dir)?;
    }
    std::fs::create_dir_all(&run_dir)?;

    let traits = dna
        .genome
        .as_ref()
        .ok_or_else(|| anyhow!("No [genome] found"))?;
    let mut routes = HashMap::new();
    let golgi_sock_path = run_dir.join("golgi.sock");

    // 1. Create Shutdown Channel
    let (shutdown_tx, _) = broadcast::channel::<()>(1);

    // 2. Create Task Tracker
    let mut monitors = JoinSet::new();

    let replicas = traits.replicas.unwrap_or(1);

    if replicas > 1 {
        sys_log("INFO", &format!("Spawning Colony: {} workers.", replicas));
        let socket_dir = run_dir.join("sockets");
        std::fs::create_dir_all(&socket_dir)?;

        let log_path = run_dir.join("service.log");
        let vacuole = Arc::new(vacuole::Vacuole::new(log_path).await?);
        let mut worker_sockets = Vec::new();

        for i in 0..replicas {
            let worker_dir = run_dir.join("workers").join(i.to_string());
            std::fs::create_dir_all(&worker_dir)?;
            let sock_path = worker_dir.join("cell.sock");
            worker_sockets.push(sock_path.clone());

            let mut guard = nucleus::activate(
                &sock_path,
                nucleus::LogStrategy::Piped,
                &bin_path,
                &golgi_sock_path,
            )?;

            let (out, err) = guard.take_pipes();
            let id = format!("w-{}", i);
            vacuole.attach(id.clone(), out, err);

            let v = vacuole.clone();
            let rx = shutdown_tx.subscribe();

            monitors.spawn(monitor_child(guard, LogTarget::Vacuole(v, id), rx));
        }
        routes.insert(
            traits.name.clone(),
            Target::LocalColony(Arc::new(worker_sockets)),
        );
    } else {
        // Single Cell Mode
        let cell_sock = run_dir.join("cell.sock");
        let log_path = run_dir.join("service.log");
        let monitor_log_path = log_path.clone();

        let guard = nucleus::activate(
            &cell_sock,
            nucleus::LogStrategy::File(log_path),
            &bin_path,
            &golgi_sock_path,
        )?;

        let rx = shutdown_tx.subscribe();
        monitors.spawn(monitor_child(guard, LogTarget::File(monitor_log_path), rx));

        routes.insert(traits.name.clone(), Target::GapJunction(cell_sock));
    }

    for (name, path) in &dna.junctions {
        routes.insert(
            name.clone(),
            Target::GapJunction(dir.join(path).join("run/cell.sock")),
        );
    }
    for (name, addr) in &dna.axons {
        let clean = addr.replace("axon://", "");
        routes.insert(
            name.clone(),
            Target::AxonCluster(vec![AxonTerminal {
                id: "static".into(),
                addr: clean,
                rtt: Duration::from_secs(1),
                last_seen: Instant::now(),
                is_donor: false,
            }]),
        );
    }

    let golgi = Golgi::new(
        traits.name.clone(),
        &run_dir,
        traits.listen.clone(),
        routes,
        is_donor,
    )?;

    tokio::select! {
        res = golgi.run() => {
            if let Err(e) = res { sys_log("CRITICAL", &format!("Golgi crashed: {}", e)); }
            let _ = shutdown_tx.send(()); // Kill children
        },
        _ = tokio::signal::ctrl_c() => {
            sys_log("INFO", "Apoptosis triggered. Shutting down cells...");
            let _ = shutdown_tx.send(());
            while let Some(_) = monitors.join_next().await {}
            sys_log("INFO", "Shutdown complete.");
        }
    }
    Ok(())
}

enum LogTarget {
    Vacuole(Arc<vacuole::Vacuole>, String),
    File(PathBuf),
}

async fn monitor_child(
    mut guard: nucleus::ChildGuard,
    target: LogTarget,
    mut shutdown_rx: broadcast::Receiver<()>,
) {
    let status_msg;

    tokio::select! {
        res = guard.wait() => {
            match res {
                Ok(stats) => {
                    if stats.exit_code == Some(0) {
                        status_msg = format!(
                            "Process exited cleanly. CPU: {}ms, RAM: {}KB, Wall: {}ms",
                            stats.cpu_time_ms, stats.max_rss_kb, stats.wall_time_ms
                        );
                    } else {
                        status_msg = format!(
                            "CRITICAL: Process crashed. Code: {:?}. CPU: {}ms",
                            stats.exit_code, stats.cpu_time_ms
                        );
                    }
                }
                Err(e) => {
                    status_msg = format!("Supervisor Error: Failed to wait on child: {}", e);
                }
            }
        }
        _ = shutdown_rx.recv() => {
            let _ = guard.kill().await;
            match guard.wait().await {
                Ok(stats) => {
                    status_msg = format!("Shutdown by Supervisor. Partial CPU: {}ms", stats.cpu_time_ms);
                }
                Err(e) => {
                    status_msg = format!("Shutdown by Supervisor (Wait failed: {})", e);
                }
            }
        }
    }

    match target {
        LogTarget::Vacuole(v, id) => {
            v.log(&id, &status_msg).await;
        }
        LogTarget::File(path) => {
            if let Ok(mut file) = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .await
            {
                let timestamp = humantime::format_rfc3339_seconds(SystemTime::now());
                let line = format!("[{}] [SUPERVISOR] {}\n", timestamp, status_msg);
                let _ = file.write_all(line.as_bytes()).await;
            }
        }
    }
}
