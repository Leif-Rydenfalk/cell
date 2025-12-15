// cell-cli/src/main.rs
// The Plumber. Creates the FIFO topology.

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
    let io_dir = run_dir.join("io"); // Where MY pipes live

    std::fs::create_dir_all(&neighbors_dir)?;
    std::fs::create_dir_all(&io_dir)?;

    // 1. Create MY IO Pipes (Where I listen)
    // Actually, cells connect peer-to-peer pipes. There isn't necessarily a central "listener"
    // in the pure P2P file model unless implemented that way.
    // BUT to support "Router" behavior, we likely want a standard interface for dynamic connections.
    // However, the prompt specifies direct file communication defined by neighbors.

    // We will create pipes for each configured neighbor.

    for (neighbor_name, config) in &manifest.neighbors {
        let neighbor_path_str = match config {
            NeighborConfig::Path(p) => p,
            NeighborConfig::Detailed { path, .. } => path,
        };

        // Paths
        let my_link_dir = neighbors_dir.join(neighbor_name);
        std::fs::create_dir_all(&my_link_dir)?;

        // Physical Pipe location: We need a shared place.
        // We use the SOURCE directory of the NEIGHBOR to find *their* runtime?
        // No, we need a neutral ground or shared knowledge.
        // Convention: The pipes live in the "server's" runtime dir if one acts as server?
        // Or we create them in a shared temp dir?
        // We are isolated in .cell/run/<instance>.

        // Assumption: All cells in an instance share the filesystem view.
        // We put pipes in the runtime dir of the TARGET (Neighbor).

        let neighbor_source_path = abs_path.join(neighbor_path_str);
        let neighbor_run_dir = neighbor_source_path.join(".cell/run").join(&instance);

        // We can't write to neighbor dir if it doesn't exist yet (autostart race).
        // Solution: Create pipes in OUR dir and symlink? Or a stable shared dir?
        // Let's create pipes in the directory of the "alphabetically first" cell to break symmetry?
        // Or simpler: Just create them in MY directory and assume neighbor links to ME?
        // That requires neighbor to know about ME.

        // Better: The "Router" or "Target" model.
        // If I depend on "db", I expect "db" to have an interface.
        // "db" should create a "gateway" pipe pair for incoming connections?
        // No, pipes are 1:1.

        // Let's implement: I create pipes in MY runtime dir for this connection.
        // I wait for Neighbor to attach.
        // BUT Neighbor needs to know where to attach.

        // REVISED TOPOLOGY:
        // We assume a 'Router' cell exists that scans dirs.
        // But for direct P2P 'db' dependency:
        // We use the Target's IO directory.
        // Target: `db/.cell/run/<inst>/io/`
        // We create: `db/.cell/run/<inst>/io/<my_name>.in` and `<my_name>.out`?
        // That requires `db` to scan its IO dir.

        // Let's implement the Scanner model for the Router/Target.
        // 1. Ensure Target IO dir exists.
        let target_io = neighbor_run_dir.join("io");
        std::fs::create_dir_all(&target_io)?; // We create it if missing

        // 2. Create pipes there: <my_name>_to_target, target_to_<my_name>
        let pipe_out = target_io.join(format!("{}_in", name)); // Write here
        let pipe_in = target_io.join(format!("{}_out", name)); // Read here

        create_fifo(&pipe_out)?;
        create_fifo(&pipe_in)?;

        // 3. Symlink locally for ME to use
        // neighbors/db/tx -> target_io/<me>_in
        // neighbors/db/rx -> target_io/<me>_out

        let my_tx = my_link_dir.join("tx");
        let my_rx = my_link_dir.join("rx");

        link_file(&pipe_out, &my_tx)?;
        link_file(&pipe_in, &my_rx)?;

        println!("   ├─ Plumbed pipe to '{}': {:?}", neighbor_name, target_io);
    }

    // 5. Build
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

    // 6. Spawn
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
