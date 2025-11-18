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
    Start {
        /// Path to cell directory (must contain cell.toml)
        dir: PathBuf,
    },
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
        name: String, // local name
    },
    /// Garbage-collect unused foreign mirrors
    Gc,
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
    }
}

// ---------- START ----------
fn cmd_start(dir: &Path) -> Result<()> {
    let mf = read_manifest(dir)?;
    let run_dir = dir.join("run");
    let log_dir = dir.join("log");
    let cache_dir = dir.join("cache");
    fs::create_dir_all(&run_dir)?;
    fs::create_dir_all(&log_dir)?;
    fs::create_dir_all(&cache_dir)?;

    let pid_file = run_dir.join("pid");
    if pid_file.exists() {
        let old = fs::read_to_string(&pid_file)?.trim().parse::<i32>()?;
        if unsafe { libc::kill(old, 0) } == 0 {
            bail!("already running (pid {})", old);
        }
    }

    let bin_path = dir.join(&mf.cell.binary);
    if !bin_path.is_file() {
        bail!("binary not found: {}", bin_path.display());
    }

    let sock_path = run_dir.join("cell.sock");
    let log_path = log_dir.join("cell.log");

    let mut cmd = Command::new(&bin_path);
    cmd.env("CELL_SOCKET_PATH", &sock_path)
        .stdout(Stdio::from(fs::File::create(&log_path)?))
        .stderr(Stdio::from(fs::File::create(&log_path)?))
        .current_dir(dir);

    let child = cmd.spawn()?;
    fs::write(pid_file, child.id().to_string())?;

    println!("✓ Started {} (pid {})", mf.cell.name, child.id());
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

// ---------- USE  (race + auto-clone) ----------
fn cmd_use(dir: &Path, fn_name: &str, args: &str) -> Result<()> {
    let mf = read_manifest(dir)?;
    let locals = vec![dir.join("run/cell.sock")];
    let foreign = discover_foreign(&mf.cell.name);

    // 1. race locals + foreign
    let winner = race_candidates(&mf.cell.name, locals.into_iter().chain(foreign))?;
    if winner.is_foreign && p99_above_budget(&winner, 150) {
        // 2. SLO breach → auto-clone once
        if let Some(repo) = &winner.repo {
            println!("🌍 SLO breach → cloning {}", repo);
            let clone_dir = foreign_mirror_dir(&mf.cell.name, repo);
            cmd_clone(repo, &mf.cell.name)?;
            // re-race including the new mirror
            let new_foreign = vec![clone_dir.join("run/cell.sock")];
            let winner = race_candidates(&mf.cell.name, new_foreign.into_iter())?;
        }
    }

    // 3. invoke
    let req_json = if args == "-" {
        std::io::read_to_string(std::io::stdin())?
    } else {
        args.into()
    };
    let resp = unix_rpc(&winner.socket, &req_json)?;
    println!("{}", resp);
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

/// Clone or pull a remote cell repo and **guarantee** an executable at the
/// location declared in cell.toml (build if missing).
fn pull_or_build_remote(name: &str, spec: &str) -> Result<()> {
    let (repo, rev) = spec
        .strip_prefix("gh:")
        .ok_or_else(|| anyhow!("only gh:owner/repo[@ref] supported"))?
        .split_once('@')
        .unwrap_or((spec, "main"));

    let root = foreign_mirror_dir(name, spec);
    fs::create_dir_all(&root)?;

    // 1.  Clone or pull
    if root.join(".git").exists() {
        println!("   Action: repo exists – pulling latest");
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
        println!("   Action: cloning {}", repo);
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

    // 2.  Read manifest to know where the binary must appear
    let manifest = read_manifest(&root)?;
    let bin_rel = PathBuf::from(&manifest.cell.binary);
    let bin_abs = root.join(&bin_rel);

    // 3.  Fast path: already executable → done
    if bin_abs.is_file() && is_executable(&bin_abs) {
        println!("   Found executable {} – skipping build", bin_rel.display());
    } else {
        // 4.  Build path
        println!("   No executable – building …");
        build_in_place(&root, &bin_rel)?;
    }

    // 5.  Start nucleus (keeps socket open, execs real binary on first conn)
    nucleus_start(&root)?;
    Ok(())
}

/// Return true if path is executable by owner.
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata()
        .map(|m| m.permissions().mode() & 0o100 != 0)
        .unwrap_or(false)
}

/// Build source however the repo wants, **then** place the final artefact
/// exactly at `bin_rel` (relative to repo root).
fn build_in_place(root: &Path, bin_rel: &Path) -> Result<()> {
    // 1.  Detect build system
    if root.join("Cargo.toml").exists() {
        println!("   Building Rust project …");
        run_command(
            Command::new("cargo")
                .args(&["build", "--release"])
                .current_dir(root)
                .stdout(Stdio::null()),
            "cargo build --release",
        )?;
        // cargo always puts binary in target/release/<name>
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

// ---------- CLONE ----------
fn cmd_clone(repo_spec: &str, name: &str) -> Result<()> {
    let (repo, rev) = repo_spec
        .strip_prefix("gh:")
        .ok_or_else(|| anyhow!("only gh:owner/repo@ref supported"))?
        .split_once('@')
        .unwrap_or((repo_spec, "main"));

    let root = foreign_mirror_dir(name, repo_spec);
    if root.exists() {
        println!("already cloned → pull only");
        Command::new("git")
            .args(&["-C", root.to_str().unwrap(), "fetch", "origin"])
            .status()?;
        Command::new("git")
            .args(&[
                "-C",
                root.to_str().unwrap(),
                "reset",
                "--hard",
                &format!("origin/{}", rev),
            ])
            .status()?;
    } else {
        fs::create_dir_all(&root)?;
        Command::new("git")
            .args(&[
                "clone",
                &format!("https://github.com/{}", repo),
                root.to_str().unwrap(),
            ])
            .status()?;
    }
    // build
    let mf = read_manifest(&root)?;
    let bin_path = root.join(&mf.cell.binary);
    if !bin_path.exists() {
        Command::new("cargo")
            .args(&["build", "--release"])
            .current_dir(&root)
            .status()?;
    }
    // start nucleus (keeps socket open, execs real binary on first conn)
    nucleus_start(&root)?;
    println!("✓ Foreign mirror ready at {}", root.display());
    Ok(())
}

// ---------- GC ----------
fn cmd_gc() -> Result<()> {
    let foreign_root = foreign_root();
    if !foreign_root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(foreign_root)? {
        let e = entry?;
        let run = e.path().join("run/pid");
        let stale = run.exists()
            && UnixStream::connect(e.path().join("run/cell.sock")).is_err()
            && fs::metadata(&run)?.modified()?.elapsed()? > Duration::from_secs(3600);
        if stale {
            fs::remove_dir_all(e.path())?;
            println!("gc {}", e.path().display());
        }
    }
    Ok(())
}

// ---------- UTILS ----------
fn read_manifest(dir: &Path) -> Result<CellManifest> {
    let txt = fs::read_to_string(dir.join("cell.toml"))?;
    toml::from_str(&txt).context("bad cell.toml")
}

fn foreign_root() -> PathBuf {
    dirs::data_local_dir().unwrap().join("cell/foreign")
}

fn foreign_mirror_dir(name: &str, repo: &str) -> PathBuf {
    let safe = repo.replace('/', "-").replace(':', "-");
    foreign_root().join(format!("{}-{}", name, safe))
}

fn discover_foreign(name: &str) -> Vec<PathBuf> {
    let root = foreign_root();
    if !root.exists() {
        return vec![];
    }
    let mut out = vec![];
    for e in fs::read_dir(root).into_iter().flatten() {
        if let Ok(e) = e {
            let sock = e.path().join("run/cell.sock");
            if sock.exists() {
                out.push(sock);
            }
        }
    }
    out
}

#[derive(Debug)]
struct Candidate {
    socket: PathBuf,
    p99_us: u64,
    is_foreign: bool,
    repo: Option<String>,
}

fn race_candidates(name: &str, socks: impl Iterator<Item = PathBuf>) -> Result<Candidate> {
    use std::sync::mpsc;
    let (tx, rx) = mpsc::channel();
    for sock in socks {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let us = ping_us(&sock).unwrap_or(u64::MAX);
            tx.send(Candidate {
                socket: sock,
                p99_us: us,
                is_foreign: true, // simplified
                repo: None,
            })
            .ok();
        });
    }
    drop(tx);
    let mut best = Candidate {
        socket: PathBuf::new(),
        p99_us: u64::MAX,
        is_foreign: false,
        repo: None,
    };
    while let Ok(c) = rx.recv_timeout(Duration::from_millis(200)) {
        if c.p99_us < best.p99_us {
            best = c;
        }
    }
    if best.p99_us == u64::MAX {
        bail!("no reachable candidate");
    }
    Ok(best)
}

fn ping_us(sock: &Path) -> Result<u64> {
    let start = std::time::Instant::now();
    let resp = unix_rpc(sock, r#"{"bench":true}"#)?;
    Ok(start.elapsed().as_micros() as u64)
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

fn p99_above_budget(c: &Candidate, budget_ms: u64) -> bool {
    c.p99_us > budget_ms * 1000
}

fn nucleus_start(dir: &Path) -> Result<()> {
    // tiny wrapper that keeps socket open and execs real binary on first conn
    let run = dir.join("run");
    fs::create_dir_all(&run)?;
    let sock = run.join("cell.sock");
    let pid_file = run.join("pid");
    if pid_file.exists() {
        return Ok(()); // already running
    }
    let mut cmd = Command::new("cell-nucleus"); // shipped with cell-sdk
    cmd.arg(&sock)
        .arg(&dir.join("bin/calculator"))
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}
