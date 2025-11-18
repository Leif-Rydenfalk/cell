mod nucleus;

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

#[derive(Parser)]
#[command(name = "cell")]
#[command(about = "Cell-native orchestrator (directory-centric MVP)")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run (build if needed) and start the cell inside its own directory
    Run {
        /// Path to cell directory (must contain cell.toml)
        dir: PathBuf,
    },
    /// Stop the cell (send SIGTERM, clean run/, keep rest)
    Stop { dir: PathBuf },
    /// Use a cell (invoke a function via Unix socket)
    Use {
        dir: PathBuf,
        fn_name: String,
        /// JSON args (or "-" to read stdin)
        args: String,
    },
    /// Garbage-collect unused cells (no-op in this version)
    Gc,
    /// Internal: wrap a cell binary (used by cell run)
    Nucleus { socket: PathBuf, binary: PathBuf },
}

// ---------- DATA ----------
#[derive(Deserialize, Debug)]
struct CellManifest {
    cell: CellMeta,
    #[serde(default)]
    deps: HashMap<String, String>,
    #[serde(default)]
    life_cycle: LifeCycle,
    #[serde(default)]
    artefact: Artefact,
}

#[derive(Deserialize, Debug)]
struct CellMeta {
    name: String,
    binary: String,
    #[serde(default = "default_true")]
    schema: bool,
}

#[derive(Deserialize, Debug, Default)]
struct LifeCycle {
    idle_timeout: Option<u64>, // seconds
    auto_cleanup: Option<bool>,
}

#[derive(Deserialize, Debug, Default)]
struct Artefact {
    artefact_type: Option<String>, // "source" | "binary"
}

fn default_true() -> bool {
    true
}

// ---------- MAIN ----------
fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Run { dir } => cmd_run(&dir),
        Commands::Stop { dir } => cmd_stop(&dir),
        Commands::Use { dir, fn_name, args } => cmd_use(&dir, &fn_name, &args),
        Commands::Gc => cmd_gc(),
        Commands::Nucleus { socket, binary } => nucleus::run_nucleus(&socket, &binary),
    }
}

/// Run = 1. build if missing  2. spawn nucleus  3. nucleus execs real binary
fn cmd_run(dir: &Path) -> Result<()> {
    let mf = read_manifest(dir)?;
    let run_dir = dir.join("run");
    let sock_path = run_dir.join("cell.sock");
    let bin_path = dir.join(&mf.cell.binary);

    fs::create_dir_all(&run_dir)?;

    // 1.  guarantee executable (build if missing)
    if !bin_path.is_file() {
        println!("🔨  Building …");
        build_in_place(dir, Path::new(&mf.cell.binary))?;
    }

    // 2.  spawn nucleus (same binary, sub-command)
    let current_exe = std::env::current_exe()?;
    let log_file = fs::File::create(run_dir.join("nucleus.log"))?;
    let mut cmd = Command::new(current_exe);
    cmd.arg("nucleus")
        .arg(&sock_path)
        .arg(&fs::canonicalize(&bin_path)?)
        .current_dir(dir)
        .stdout(Stdio::inherit())
        .stderr(Stdio::from(log_file.try_clone()?));

    let mut child = cmd.spawn().context("spawn nucleus failed")?;

    // 3.  wait until socket appears (or die with stderr)
    for _ in 0..50 {
        if sock_path.exists() {
            println!("✓ Started {} (nucleus pid {})", mf.cell.name, child.id());
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let stderr = fs::read_to_string(run_dir.join("nucleus.log"))?;
    bail!("nucleus failed to create socket:\n{}", stderr)
}

// ---------- STOP ----------
fn cmd_stop(dir: &Path) -> Result<()> {
    let pid_file = dir.join("run/pid");
    if !pid_file.exists() {
        bail!("not running");
    }
    let pid = fs::read_to_string(&pid_file)?.trim().parse::<i32>()?;
    unsafe { libc::kill(pid, libc::SIGTERM) };
    fs::remove_file(pid_file)?;
    println!("✓ Stopped");
    Ok(())
}

// ---------- USE ----------
fn cmd_use(dir: &Path, fn_name: &str, args: &str) -> Result<()> {
    let _mf = read_manifest(dir)?; // ensure dir is a cell
    let sock = dir.join("run/cell.sock");
    let req_json = if args == "-" {
        std::io::read_to_string(std::io::stdin())?
    } else {
        args.into()
    };
    let resp = unix_rpc(&sock, &req_json)
        .with_context(|| format!("cannot connect to socket {}", sock.display()))?;
    println!("{}", resp);
    Ok(())
}

// ---------- BUILD ----------
fn build_in_place(root: &Path, bin_rel: &Path) -> Result<()> {
    if root.join("Cargo.toml").exists() {
        println!("   Building Rust project …");
        run_command(
            Command::new("cargo")
                .args(&["build", "--release"])
                .current_dir(root)
                .stdout(Stdio::null()),
            "cargo build --release",
        )?;
        let name = bin_rel
            .file_name()
            .ok_or_else(|| anyhow!("binary path has no file name"))?;
        let cargo_out = root.join("target/release").join(name);
        let wanted = root.join(bin_rel);
        if !cargo_out.exists() {
            bail!("cargo succeeded but {} not found", cargo_out.display());
        }
        fs::create_dir_all(wanted.parent().unwrap())?;
        fs::copy(&cargo_out, &wanted)?;
        println!("   Copied {} → {}", cargo_out.display(), wanted.display());
    } else if root.join("Makefile").exists() {
        println!("   Building with Makefile …");
        run_command(Command::new("make").current_dir(root), "make")?
    } else if root.join("build.sh").exists() {
        println!("   Running build.sh …");
        run_command(Command::new("./build.sh").current_dir(root), "./build.sh")?
    } else {
        bail!("no build recipe (Cargo.toml, Makefile, build.sh)")
    }
    Ok(())
}

// ---------- UTILS ----------
fn read_manifest(dir: &Path) -> Result<CellManifest> {
    let txt = fs::read_to_string(dir.join("cell.toml"))?;
    toml::from_str(&txt).context("bad cell.toml")
}

fn run_command(cmd: &mut Command, desc: &str) -> Result<()> {
    println!("   > {}", format!("{:?}", cmd).replace('"', ""));
    let status = cmd
        .status()
        .with_context(|| format!("Failed to execute: {}", desc))?;
    if !status.success() {
        bail!("Command failed with status {}: {}", status, desc);
    }
    Ok(())
}

fn unix_rpc(sock: &Path, req: &str) -> Result<String> {
    let mut s = UnixStream::connect(sock)?;
    let req = req.as_bytes();
    s.write_all(&(req.len() as u32).to_be_bytes())?;
    s.write_all(req)?;
    s.flush()?;
    let mut len_buf = [0u8; 4];
    s.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    s.read_exact(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn cmd_gc() -> Result<()> {
    // no-op: cells live in user-controlled directories
    Ok(())
}
