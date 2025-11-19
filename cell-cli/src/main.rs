mod nucleus;
mod router;

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
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Run {
        dir: PathBuf,
    },
    Stop {
        dir: PathBuf,
    },
    Use {
        dir: PathBuf,
        fn_name: String,
        args: String,
    },
    Nucleus {
        socket: PathBuf,
        binary: PathBuf,
        router: PathBuf,
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
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Run { dir } => cmd_run(&dir).await,
        Commands::Stop { dir } => cmd_stop(&dir),
        Commands::Use {
            dir,
            fn_name: _,
            args,
        } => cmd_use(&dir, &args),
        Commands::Nucleus {
            socket,
            binary,
            router,
        } => nucleus::run_nucleus(&socket, &binary, &router),
    }
}

async fn cmd_run(dir: &Path) -> Result<()> {
    let dir = dir.canonicalize()?;
    let mf = read_manifest(&dir)?;
    let run_dir = dir.join("run");
    fs::create_dir_all(&run_dir)?;

    // 1. Dependencies Snapshot (For Macros)
    if !mf.deps.is_empty() {
        snapshot_dependencies(&dir, &mf.deps)?;
    }

    // 2. Build
    let bin_path = dir.join(&mf.cell.binary);
    if !bin_path.is_file() {
        println!("🔨 Building...");
        build_in_place(&dir, Path::new(&mf.cell.binary))?;
    }

    // 3. Configure Router
    let mut router = router::Router::new(&run_dir);
    let router_sock = run_dir.join("router.sock");

    if let Some(parent) = dir.parent() {
        for (dep_name, _) in &mf.deps {
            // Default to local sibling
            let sibling_sock = parent.join(dep_name).join("run/cell.sock");
            router.add_local_route(dep_name.clone(), sibling_sock);
        }
    }

    let router_handle = tokio::spawn(async move {
        let _ = router.serve().await;
    });

    // 4. Spawn Nucleus
    let current_exe = std::env::current_exe()?;
    let log_file = fs::File::create(run_dir.join("nucleus.log"))?;
    let sock_path = run_dir.join("cell.sock");

    let mut cmd = Command::new(current_exe);
    cmd.arg("nucleus")
        .arg(&sock_path)
        .arg(&bin_path)
        .arg(&router_sock)
        .current_dir(&dir)
        .stdout(Stdio::null())
        .stderr(Stdio::from(log_file));

    let mut child = cmd.spawn()?;
    fs::write(run_dir.join("pid"), child.id().to_string())?;

    println!("🚀 {} started (PID: {}).", mf.cell.name, child.id());
    println!("   Press Ctrl+C to stop.");

    tokio::select! {
        _ = router_handle => {},
        _ = tokio::signal::ctrl_c() => {
            let _ = unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
        }
    }
    Ok(())
}

// --- UTILS ---

fn snapshot_dependencies(cell_dir: &Path, deps: &HashMap<String, String>) -> Result<()> {
    let schema_dir = cell_dir.join(".cell-schemas");
    fs::create_dir_all(&schema_dir)?;
    // In a real mesh, we would query the router. For now, we check local disk.
    let parent = cell_dir.parent().ok_or_else(|| anyhow!("No parent"))?;
    for (service, _) in deps {
        let sock = parent.join(service).join("run/cell.sock");
        if sock.exists() {
            // Quick sync connection
            if let Ok(mut stream) = UnixStream::connect(&sock) {
                let req = b"__SCHEMA__";
                stream.write_all(&(req.len() as u32).to_be_bytes())?;
                stream.write_all(req)?;
                let mut len_buf = [0u8; 4];
                stream.read_exact(&mut len_buf)?;
                let len = u32::from_be_bytes(len_buf) as usize;
                let mut buf = vec![0u8; len];
                stream.read_exact(&mut buf)?;
                fs::write(schema_dir.join(format!("{}.json", service)), buf)?;
            }
        }
    }
    Ok(())
}

fn read_manifest(dir: &Path) -> Result<CellManifest> {
    let txt = fs::read_to_string(dir.join("cell.toml"))?;
    toml::from_str(&txt).context("Invalid TOML")
}

fn build_in_place(root: &Path, bin_rel: &Path) -> Result<()> {
    let st = Command::new("cargo")
        .arg("build")
        .arg("--release")
        .current_dir(root)
        .stdout(Stdio::null())
        .status()?;
    if !st.success() {
        bail!("Build failed");
    }

    // Find workspace target or local target
    let local_tgt = root
        .join("target/release")
        .join(bin_rel.file_name().unwrap());
    if local_tgt.exists() {
        fs::create_dir_all(root.join(bin_rel).parent().unwrap())?;
        fs::copy(local_tgt, root.join(bin_rel))?;
        return Ok(());
    }

    // Try workspace root
    let mut up = root.to_path_buf();
    while let Some(p) = up.parent() {
        let ws_tgt = p.join("target/release").join(bin_rel.file_name().unwrap());
        if ws_tgt.exists() {
            fs::create_dir_all(root.join(bin_rel).parent().unwrap())?;
            fs::copy(ws_tgt, root.join(bin_rel))?;
            return Ok(());
        }
        up = p.to_path_buf();
    }
    bail!("Could not find built binary");
}

fn cmd_stop(dir: &Path) -> Result<()> {
    let pid_path = dir.join("run/pid");
    if pid_path.exists() {
        let pid = fs::read_to_string(&pid_path)?.trim().parse::<i32>()?;
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
        fs::remove_file(pid_path)?;
    }
    Ok(())
}

fn cmd_use(dir: &Path, args: &str) -> Result<()> {
    // For CLI usage, we connect directly to the local socket for simplicity,
    // or we could implement a mini-client that talks to router.
    // MVP: Direct socket
    let sock = dir.join("run/cell.sock");
    let mut stream = UnixStream::connect(sock)?;
    let b = args.as_bytes();
    stream.write_all(&(b.len() as u32).to_be_bytes())?;
    stream.write_all(b)?;
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut resp = vec![0u8; len];
    stream.read_exact(&mut resp)?;
    println!("{}", String::from_utf8_lossy(&resp));
    Ok(())
}
