// cell-cli/src/main.rs
// The Plumber.

use anyhow::{anyhow, Context, Result};
use cell_model::manifest::{CellManifest, NeighborConfig};
use clap::{Parser, Subcommand};
use rand::Rng;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Parser)]
#[command(name = "cell", version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Run {
        #[arg(default_value = ".")]
        path: PathBuf,
        #[arg(long)]
        release: bool,
        #[arg(long, env = "CELL_INSTANCE")]
        instance: Option<String>,
    },
    Stop {
        cell_name: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Run {
            path,
            release,
            instance,
        } => cmd_run(path, release, instance).await,
        Commands::Stop { cell_name } => cmd_stop(cell_name).await,
    }
}

async fn cmd_run(path: PathBuf, release: bool, instance_id: Option<String>) -> Result<()> {
    let abs_path = std::fs::canonicalize(&path).context("Invalid cell path")?;
    let manifest = read_manifest(&abs_path)?;
    let name = get_cell_name(&manifest)?;

    let instance = instance_id.unwrap_or_else(|| {
        let mut rng = rand::thread_rng();
        format!("{:08x}", rng.gen::<u32>())
    });

    println!("🧬 Igniting Cell: {} (Instance: {})", name, instance);

    let cell_root = abs_path.join(".cell");
    let run_dir = cell_root.join("run").join(&instance);
    let neighbors_dir = run_dir.join("neighbors");
    let io_dir = run_dir.join("io");

    std::fs::create_dir_all(&neighbors_dir)?;
    std::fs::create_dir_all(&io_dir)?;

    // WIRE NEIGHBORS
    // This creates the pipe topology based on the Manifest.
    for (neighbor_name, config) in &manifest.neighbors {
        let neighbor_path_str = match config {
            NeighborConfig::Path(p) => p,
            NeighborConfig::Detailed { path, .. } => path,
        };

        let my_link_dir = neighbors_dir.join(neighbor_name);
        std::fs::create_dir_all(&my_link_dir)?;

        let neighbor_source_path = abs_path.join(neighbor_path_str);
        let neighbor_run_dir = neighbor_source_path.join(".cell/run").join(&instance);

        let target_io = neighbor_run_dir.join("io");
        std::fs::create_dir_all(&target_io)?;

        // Create Pipes
        // <my_name>_in  (I write here, they read)
        // <my_name>_out (I read here, they write)
        let pipe_out = target_io.join(format!("{}_in", name));
        let pipe_in = target_io.join(format!("{}_out", name));

        create_fifo(&pipe_out)?;
        create_fifo(&pipe_in)?;

        // Link to my view
        let my_tx = my_link_dir.join("tx");
        let my_rx = my_link_dir.join("rx");

        link_file(&pipe_out, &my_tx)?;
        link_file(&pipe_in, &my_rx)?;

        println!("   ├─ Plumbed pipe to '{}': {:?}", neighbor_name, target_io);
    }

    // BUILD
    let mut build = Command::new("cargo");
    build.arg("build");
    if release {
        build.arg("--release");
    }
    build.current_dir(&abs_path);
    build.stdout(Stdio::inherit());
    build.stderr(Stdio::inherit());
    if !build.status()?.success() {
        anyhow::bail!("Ribosome failed to compile DNA");
    }

    // SPAWN
    let target_dir = abs_path
        .join("target")
        .join(if release { "release" } else { "debug" });
    let binary = target_dir.join(&name);

    if !binary.exists() {
        anyhow::bail!("Binary not found");
    }

    println!("⚡ Spark of Life...");

    let mut child = Command::new(&binary);
    child.env("CELL_SOCKET_DIR", &run_dir);
    child.env("CELL_NAME", &name);
    child.env("CELL_INSTANCE", &instance);

    child.stdin(Stdio::inherit());
    child.stdout(Stdio::inherit());
    child.stderr(Stdio::inherit());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        child.exec();
    }

    #[cfg(windows)]
    {
        let mut handle = child.spawn()?;
        handle.wait()?;
    }
    Ok(())
}

fn create_fifo(path: &Path) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use nix::sys::stat;
        nix::unistd::mkfifo(path, stat::Mode::S_IRWXU)?;
    }
    // Windows named pipes are created at open time usually,
    // but for filesystem mapping we might need a placeholder or driver.
    // For MVP, Unix FIFO is the standard.
    Ok(())
}

fn link_file(src: &Path, dst: &Path) -> Result<()> {
    if dst.exists() {
        std::fs::remove_file(dst)?;
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(src, dst)?;
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(src, dst)?;
    Ok(())
}

fn read_manifest(path: &Path) -> Result<CellManifest> {
    let toml_path = if path.join("Cell.toml").exists() {
        path.join("Cell.toml")
    } else {
        path.join("Cargo.toml")
    };
    let content = std::fs::read_to_string(&toml_path)?;
    Ok(toml::from_str(&content)?)
}

fn get_cell_name(manifest: &CellManifest) -> Result<String> {
    if let Some(c) = &manifest.cell {
        Ok(c.name.clone())
    } else if let Some(p) = &manifest.package {
        Ok(p.name.clone())
    } else {
        Err(anyhow!("No cell name"))
    }
}

async fn cmd_stop(_name: String) -> Result<()> {
    Ok(())
}
