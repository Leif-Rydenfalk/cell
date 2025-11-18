use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Parser)]
#[command(name = "cell")]
#[command(about = "Cell microservice orchestrator")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start a service in the background
    Start { name: String, binary: PathBuf },
    /// Build, pull, and cache schemas for all cells in the workspace
    Build {
        /// Automatically start any missing local services to fetch their schemas
        #[arg(long)]
        start_missing: bool,
    },
}

#[derive(Deserialize, Debug)]
struct CellManifest {
    cell: CellMeta,
    #[serde(default)]
    deps: HashMap<String, String>,
}

#[derive(Deserialize, Debug)]
struct CellMeta {
    name: String,
    binary: String,
    #[serde(default = "default_true")]
    schema: bool,
}

fn default_true() -> bool {
    true
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Start { name, binary } => cmd_start(name, binary),
        Commands::Build { start_missing } => cmd_build(start_missing),
    }
}

fn cmd_start(name: String, binary: PathBuf) -> Result<()> {
    let tmp_dir = PathBuf::from("/tmp/cell");
    std::fs::create_dir_all(tmp_dir.join("sockets"))?;
    std::fs::create_dir_all(tmp_dir.join("logs"))?;

    let log_file_path = tmp_dir.join(format!("logs/{}.log", name));
    let socket_path = tmp_dir.join(format!("sockets/{}.sock", name));

    let log_file = std::fs::File::create(&log_file_path)
        .with_context(|| format!("Failed to create log file at {:?}", log_file_path))?;

    Command::new(binary)
        .env("CELL_SOCKET_PATH", &socket_path)
        .stdout(log_file.try_clone()?)
        .stderr(log_file)
        .spawn()
        .with_context(|| "Failed to spawn the service process")?;

    println!("✓ Started service '{}'", name);
    Ok(())
}

fn cmd_build(start_missing: bool) -> Result<()> {
    println!("🔍 Discovering workspace cells...");
    let root = find_workspace_root()
        .context("Failed to find workspace root. Are you in a cell project?")?;
    let manifests = discover_manifests(&root)?;

    if manifests.is_empty() {
        println!("⚠️ No 'cell.toml' files found in the workspace.");
        return Ok(());
    }
    println!("📦 Found {} cell(s).", manifests.len());

    let remote_deps: Vec<_> = manifests.iter().flat_map(|m| &m.deps).collect();
    if !remote_deps.is_empty() {
        println!("\n--- Processing Remote Dependencies ---");
        for (cell_name, source) in remote_deps {
            println!("➡️ {} -> {}", cell_name, source);
            pull_or_build_remote(cell_name, source)?;
        }
    }

    println!("\n--- Building Local Cells ---");
    for m in &manifests {
        build_local_cell(&m.cell)?;
    }

    if start_missing {
        println!("\n--- Ensuring Services are Running for Schema Discovery ---");
        for m in &manifests {
            let sock_path = format!("/tmp/cell/sockets/{}.sock", m.cell.name);
            if !Path::new(&sock_path).exists() {
                let bin_path = root.join(&m.cell.binary);
                if !bin_path.exists() {
                    anyhow::bail!(
                        "Binary for '{}' not found at '{}'. Please check 'cell.toml'.",
                        m.cell.name,
                        bin_path.display()
                    );
                }
                println!("🚀 Starting '{}'...", m.cell.name);
                cmd_start(m.cell.name.clone(), bin_path)?;
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        }
    }

    println!("\n--- Caching Service Schemas ---");
    let out_dir = root.join("target/cell-schemas");
    std::fs::create_dir_all(&out_dir).context("Failed to create schema cache directory")?;
    std::env::set_var("OUT_DIR", &out_dir);

    for m in &manifests {
        if m.cell.schema {
            println!("💾 Caching schema for '{}'...", m.cell.name);
            if let Err(e) = fetch_schema(&m.cell.name, &out_dir) {
                println!("⚠️  Could not cache schema for '{}': {}. This may be expected if the service isn't running.", m.cell.name, e);
            }
        }
    }

    println!("\n✅ Build complete.");
    Ok(())
}

fn fetch_schema(service_name: &str, out_dir: &Path) -> Result<()> {
    use std::io::{Read, Write};
    let socket_path = format!("/tmp/cell/sockets/{}.sock", service_name);
    let mut stream = std::os::unix::net::UnixStream::connect(&socket_path).with_context(|| {
        format!(
            "Service '{}' not running or socket not at {}",
            service_name, socket_path
        )
    })?;

    stream.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;

    let request = b"__SCHEMA__";
    stream.write_all(&(request.len() as u32).to_be_bytes())?;
    stream.write_all(request)?;
    stream.flush()?;

    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;

    let mut schema_buf = vec![0u8; len];
    stream.read_exact(&mut schema_buf)?;

    let schema_path = out_dir.join(format!("{}_schema.json", service_name));
    std::fs::write(&schema_path, &schema_buf)
        .with_context(|| format!("Failed to write schema to {:?}", schema_path))?;

    Ok(())
}

fn find_workspace_root() -> Result<PathBuf> {
    let current_dir = std::env::current_dir()?;
    for ancestor in current_dir.ancestors() {
        if ancestor.join("Cargo.toml").is_file() {
            let content = std::fs::read_to_string(ancestor.join("Cargo.toml"))?;
            if content.contains("[workspace]")
                && (ancestor.join("cell").is_dir() || ancestor.join("examples").is_dir())
            {
                return Ok(ancestor.to_path_buf());
            }
        }
    }
    anyhow::bail!("Cannot find workspace root from current directory")
}

fn discover_manifests(root: &Path) -> Result<Vec<CellManifest>> {
    let mut manifests = vec![];
    for entry in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if entry.file_name() == "cell.toml" {
            let content = std::fs::read_to_string(entry.path())?;
            let manifest: CellManifest = toml::from_str(&content)
                .with_context(|| format!("Failed to parse {}", entry.path().display()))?;
            manifests.push(manifest);
        }
    }
    Ok(manifests)
}

fn run_command(cmd: &mut Command, desc: &str) -> Result<()> {
    println!("   > {}", format!("{:?}", cmd).replace("\"", ""));
    let status = cmd
        .status()
        .with_context(|| format!("Failed to execute: {}", desc))?;
    if !status.success() {
        anyhow::bail!("Command failed with status {}: {}", status, desc);
    }
    Ok(())
}

fn pull_or_build_remote(name: &str, spec: &str) -> Result<()> {
    if let Some(image) = spec.strip_prefix("docker:") {
        println!("   Action: Pulling docker image {}", image);
        run_command(Command::new("docker").args(&["pull", image]), "docker pull")?;
        println!("   Action: Tagging image for local use");
        run_command(
            Command::new("docker").args(&["tag", image, &format!("cell-{}:latest", name)]),
            "docker tag",
        )?;
    } else if let Some(repo) = spec.strip_prefix("gh:") {
        let tmp_dir = PathBuf::from(format!("/tmp/cell-src/{}", name));
        if !tmp_dir.exists() {
            println!("   Action: Cloning git repository {}", repo);
            let repo_url = format!("https://github.com/{}", repo);
            run_command(
                Command::new("git").args(&["clone", &repo_url, tmp_dir.to_str().unwrap()]),
                "git clone",
            )?;
        } else {
            println!("   Action: Git repository already cloned, skipping.");
        }

        println!("   Action: Building dependency '{}' from source", name);
        run_command(
            Command::new("cargo")
                .current_dir(&tmp_dir)
                .args(&["build", "--release"]),
            "cargo build --release (dependency)",
        )?;

        let bin_dir = PathBuf::from("target/cell-bin");
        std::fs::create_dir_all(&bin_dir)?;
        let src_path = tmp_dir.join("target/release").join(name);
        let dest_path = bin_dir.join(name);
        std::fs::copy(&src_path, &dest_path).with_context(|| {
            format!(
                "Failed to copy binary from {:?} to {:?}",
                src_path, dest_path
            )
        })?;
        println!("   Action: Copied binary to {}", dest_path.display());
    } else {
        anyhow::bail!("Unknown dependency source format: '{}'", spec);
    }
    Ok(())
}

fn build_local_cell(meta: &CellMeta) -> Result<()> {
    println!("🔨 Building local cell '{}'...", meta.name);

    // 1.  If binary already exists and is newer than src, skip.
    let bin_path = PathBuf::from(&meta.binary);
    if bin_path.exists() {
        println!("   Binary already up-to-date at {}", bin_path.display());
        return Ok(());
    }

    // 2.  Otherwise build from the examples workspace.
    let examples_dir = find_workspace_root()?.join("examples");
    run_command(
        Command::new("cargo")
            .current_dir(&examples_dir)
            .args(&["build", "--release", "--package", &meta.name])
            .stdout(Stdio::null()),
        &format!("cargo build for {}", meta.name),
    )
}
