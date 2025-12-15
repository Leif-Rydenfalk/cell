// cell-cli/src/main.rs
// The Catalyst. Creates the topology and ignites the cell.

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
    /// Ignite a cell in the current directory or from a path
    Run {
        /// Path to the cell directory (default: current dir)
        #[arg(default_value = ".")]
        path: PathBuf,

        /// Run in release mode
        #[arg(long)]
        release: bool,

        /// Attach to an existing instance ID (default: new random ID)
        #[arg(long, env = "CELL_INSTANCE")]
        instance: Option<String>,
    },
    /// Stop a running cell
    Stop { cell_name: String },
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

    // 1. Read Manifest (Cell.toml or Cargo.toml)
    let manifest = read_manifest(&abs_path)?;
    let name = get_cell_name(&manifest)?;

    // 2. Determine Instance Identity
    let instance = instance_id.unwrap_or_else(|| {
        let mut rng = rand::thread_rng();
        format!("{:08x}", rng.gen::<u32>())
    });

    println!("🧬 Igniting Cell: {} (Instance: {})", name, instance);

    // 3. Prepare Runtime Environment (The Filesystem Topology)
    // Structure: .cell/run/<instance>/
    //            ├── cell.sock
    //            └── neighbors/
    //                ├── db -> ../../../../db/.cell/run/<instance>/cell.sock
    //                └── router -> ../../../../router/.cell/run/<instance>/cell.sock

    let cell_root = abs_path.join(".cell");
    let run_dir = cell_root.join("run").join(&instance);
    let neighbors_dir = run_dir.join("neighbors");

    std::fs::create_dir_all(&neighbors_dir)?;

    // 4. Wire Neighbors
    for (neighbor_name, config) in &manifest.neighbors {
        let neighbor_path_str = match config {
            NeighborConfig::Path(p) => p,
            NeighborConfig::Detailed { path, .. } => path,
        };

        let neighbor_source_path = abs_path.join(neighbor_path_str);

        // The neighbor's socket is relative to ITS runtime in the SAME instance scope
        // Assumption: Neighbors are running in the same instance context (Monorepo flow)
        let neighbor_socket = neighbor_source_path
            .join(".cell/run")
            .join(&instance)
            .join("cell.sock");

        // We link: .cell/run/<inst>/neighbors/<name> -> <neighbor_source>/.cell/run/<inst>/cell.sock
        let link_path = neighbors_dir.join(neighbor_name);
        if link_path.exists() {
            std::fs::remove_file(&link_path)?;
        }

        #[cfg(unix)]
        std::os::unix::fs::symlink(&neighbor_socket, &link_path)?;

        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&neighbor_socket, &link_path)?;

        println!("   ├─ Wired neighbor '{}'", neighbor_name);
    }

    // 5. Ribosome: Compile
    println!("Ribosome: Synthesizing proteins...");
    let mut build = Command::new("cargo");
    build.arg("build");
    if release {
        build.arg("--release");
    }
    build.current_dir(&abs_path);
    // Hide build output unless error
    build.stdout(Stdio::inherit());
    build.stderr(Stdio::inherit());

    let status = build.status()?;
    if !status.success() {
        anyhow::bail!("Ribosome failed to compile DNA");
    }

    // 6. Mitosis: Spawn the process
    let target_dir = abs_path
        .join("target")
        .join(if release { "release" } else { "debug" });
    let binary = target_dir.join(&name);

    if !binary.exists() {
        anyhow::bail!("Binary not found at {:?}", binary);
    }

    println!("⚡ Spark of Life...");

    let mut child = Command::new(&binary);
    // The CRITICAL environment variable. The SDK uses this to find itself.
    child.env("CELL_SOCKET_DIR", &run_dir);
    child.env("CELL_NAME", &name);
    child.env("CELL_INSTANCE", &instance);

    // Pass through IO
    child.stdin(Stdio::inherit());
    child.stdout(Stdio::inherit());
    child.stderr(Stdio::inherit());

    // Exec (Replace CLI process)
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = child.exec();
        anyhow::bail!("Failed to exec cell: {}", err);
    }

    #[cfg(windows)]
    {
        let mut handle = child.spawn()?;
        handle.wait()?;
        Ok(())
    }
}

fn read_manifest(path: &Path) -> Result<CellManifest> {
    let toml_path = if path.join("Cell.toml").exists() {
        path.join("Cell.toml")
    } else {
        path.join("Cargo.toml")
    };

    let content = std::fs::read_to_string(&toml_path)
        .context(format!("Failed to read manifest at {:?}", toml_path))?;

    let manifest: CellManifest = toml::from_str(&content).context("Failed to parse manifest")?;

    Ok(manifest)
}

fn get_cell_name(manifest: &CellManifest) -> Result<String> {
    if let Some(c) = &manifest.cell {
        return Ok(c.name.clone());
    }
    if let Some(p) = &manifest.package {
        return Ok(p.name.clone());
    }
    Err(anyhow!("No cell name found in manifest"))
}

async fn cmd_stop(_name: String) -> Result<()> {
    // Implementation left as exercise: Look for PID file in .cell/run
    Ok(())
}
