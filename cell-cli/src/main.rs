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
    /// Start the cell inside its own directory
    Start { dir: PathBuf },
    /// Stop the cell (send SIGTERM, clean run/, keep rest)
    Stop { dir: PathBuf },
    /// Use a cell (race local + foreign, auto-clone on SLO breach)
    Use {
        dir: PathBuf,
        fn_name: String,
        /// JSON args (or "-" to read stdin)
        args: String,
    },
    /// Clone & build a foreign cell repo once
    Clone {
        repo: String, // gh:owner/repo@ref
        name: String, // local directory name
    },
    /// Garbage-collect unused foreign mirrors
    Gc,
    /// Internal: wrap a cell binary (used by cell start)
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
        Commands::Start { dir } => cmd_start(&dir),
        Commands::Stop { dir } => cmd_stop(&dir),
        Commands::Use { dir, fn_name, args } => cmd_use(&dir, &fn_name, &args),
        Commands::Clone { repo, name } => cmd_clone(&repo, &name),
        Commands::Gc => cmd_gc(),
        Commands::Nucleus { socket, binary } => nucleus::run_nucleus(&socket, &binary),
    }
}

/// Start = 1. build if missing  2. spawn nucleus  3. nucleus execs real binary
fn cmd_start(dir: &Path) -> Result<()> {
    let mf = read_manifest(dir)?;
    let run_dir = dir.join("run");
    let sock_path = run_dir.join("cell.sock");
    let bin_path = fs::canonicalize(dir.join(&mf.cell.binary))?;

    fs::create_dir_all(&run_dir)?;

    // 1.  guarantee executable (build if missing)
    if !bin_path.is_file() {
        println!("🔨  Building …");
        build_in_place(dir, Path::new(&mf.cell.binary))?;
    }

    // 2.  spawn nucleus (same binary, sub-command)
    let current_exe = std::env::current_exe()?;
    let mut cmd = Command::new(current_exe);
    cmd.arg("nucleus")
        .arg(&sock_path)
        .arg(&bin_path)
        .current_dir(dir)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    let child = cmd.spawn()?;
    println!("✓ Started {} (nucleus pid {})", mf.cell.name, child.id());
    Ok(())
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

// ---------- USE  ----------
fn cmd_use(dir: &Path, fn_name: &str, args: &str) -> Result<()> {
    let mf = read_manifest(dir)?;
    let sock = dir.join("run/cell.sock");

    let req_json = if args == "-" {
        std::io::read_to_string(std::io::stdin())?
    } else {
        args.into()
    };

    let resp = unix_rpc(&sock, &req_json)?;
    println!("{}", resp);
    Ok(())
}

// ---------- CLONE ----------
fn cmd_clone(repo_spec: &str, name: &str) -> Result<()> {
    let (repo, rev) = repo_spec
        .strip_prefix("gh:")
        .ok_or_else(|| anyhow!("only gh:owner/repo[@ref] supported"))?
        .split_once('@')
        .unwrap_or((repo_spec, "main"));

    let root = PathBuf::from(name);
    if root.exists() {
        println!("already cloned → pull only");
        run_command(
            Command::new("git")
                .args(&["-C", root.to_str().unwrap(), "fetch", "origin"])
                .stdout(Stdio::null()),
            "git fetch",
        )?;
        run_command(
            Command::new("git")
                .args(&[
                    "-C",
                    root.to_str().unwrap(),
                    "reset",
                    "--hard",
                    &format!("origin/{}", rev),
                ])
                .stdout(Stdio::null()),
            "git reset",
        )?;
    } else {
        fs::create_dir_all(&root)?;
        run_command(
            Command::new("git")
                .args(&[
                    "clone",
                    &format!("https://github.com/{}", repo),
                    root.to_str().unwrap(),
                ])
                .stdout(Stdio::null()),
            "git clone",
        )?;
    }

    // build
    let mf = read_manifest(&root)?;
    let bin_path = root.join(&mf.cell.binary);
    if !bin_path.is_file() {
        build_in_place(&root, Path::new(&mf.cell.binary))?;
    }

    println!("✓ Cell ready at {}", root.display());
    Ok(())
}

// ---------- GC ----------
fn cmd_gc() -> Result<()> {
    // nothing to do – we no longer keep a hidden cache
    Ok(())
}

// ---------- COMMAND RUNNER ----------
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

/// Build source however the repo wants, **then** place the final artefact
/// exactly at `bin_rel` (relative to repo root).
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

/// Start the nucleus wrapper (bind socket, exec real binary on first connection).
/// Returns immediately after spawning the nucleus process.
fn nucleus_start(dir: &Path) -> Result<()> {
    let run = dir.join("run");
    fs::create_dir_all(&run)?;

    let sock = run.join("cell.sock");
    let pid_file = run.join("pid");

    // Already running?
    if pid_file.exists() {
        return Ok(());
    }

    // Path to the real service binary (relative to cell dir)
    let mf = read_manifest(dir)?;
    let real_bin = dir.join(&mf.cell.binary);

    // Re-exec *this* binary in nucleus mode
    let current_exe = std::env::current_exe().context("cannot locate own executable")?;

    let mut cmd = Command::new(current_exe);
    cmd.arg("nucleus")
        .arg(&sock)
        .arg(&real_bin)
        .current_dir(dir)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    let child = cmd.spawn().context("spawn nucleus failed")?;

    // Write PID so `cell stop` can find us
    fs::write(&pid_file, child.id().to_string())?;

    Ok(())
}
