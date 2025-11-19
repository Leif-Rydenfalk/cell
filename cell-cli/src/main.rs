mod nucleus;
mod router;

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use router::RouteTarget;
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
    network: NetworkConfig,
    #[serde(default)]
    deps: HashMap<String, String>,
}

#[derive(Deserialize, Debug)]
struct CellMeta {
    name: String,
    binary: String,
}

#[derive(Deserialize, Debug, Default)]
struct NetworkConfig {
    // e.g., "0.0.0.0:9000"
    listen: Option<String>,
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

    // 1. Snapshot Dependencies
    if !mf.deps.is_empty() {
        let _ = snapshot_dependencies(&dir, &mf.deps);
    }

    // 2. BUILD
    println!("🔨  Compiling {}...", mf.cell.name);
    let status = Command::new("cargo")
        .arg("build")
        .arg("--release")
        .current_dir(&dir)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;

    if !status.success() {
        bail!("Build failed");
    }

    let bin_name = Path::new(&mf.cell.binary)
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new(&mf.cell.name));
    let build_artifact = find_build_artifact(&dir, bin_name)?;
    let runtime_binary = run_dir.join(bin_name);

    if runtime_binary.exists() {
        fs::remove_file(&runtime_binary).context("Failed to remove old binary")?;
    }
    fs::copy(&build_artifact, &runtime_binary).context("Failed to copy binary")?;

    // 3. Configure Router
    let mut router = router::Router::new(&run_dir, mf.network.listen.clone());
    let router_sock = run_dir.join("router.sock");

    // Register the Cell's own name pointing to its own socket
    // This allows remote peers to ask for "worker" and get routed to cell.sock
    let self_sock = run_dir.join("cell.sock");
    router.add_route(mf.cell.name.clone(), RouteTarget::LocalUnix(self_sock));

    // Process Routes (Dependencies)
    if let Some(parent) = dir.parent() {
        for (dep_name, path_or_url) in &mf.deps {
            if path_or_url.starts_with("tcp://") {
                let addr = path_or_url.strip_prefix("tcp://").unwrap();
                router.add_route(dep_name.clone(), RouteTarget::RemoteTcp(addr.to_string()));
                println!("   → Remote Route: {} -> {}", dep_name, addr);
            } else {
                let sibling_sock = parent.join(path_or_url).join("run/cell.sock");
                router.add_route(dep_name.clone(), RouteTarget::LocalUnix(sibling_sock));
            }
        }
    }

    let router_handle = tokio::spawn(async move {
        if let Err(e) = router.serve().await {
            eprintln!("Router error: {}", e);
        }
    });

    // 4. Spawn Nucleus
    let current_exe = std::env::current_exe()?;
    let log_file = fs::File::create(run_dir.join("nucleus.log"))?;
    let sock_path = run_dir.join("cell.sock");

    let mut cmd = Command::new(current_exe);
    cmd.arg("nucleus")
        .arg(&sock_path)
        .arg(&runtime_binary)
        .arg(&router_sock)
        .current_dir(&dir)
        .stdout(Stdio::null())
        .stderr(Stdio::from(log_file));

    let mut child = cmd.spawn()?; // Removed 'mut'
    fs::write(run_dir.join("pid"), child.id().to_string())?;

    println!("🚀 {} started (PID: {}).", mf.cell.name, child.id());
    if let Some(l) = mf.network.listen {
        println!("   🌐 Accepting TCP on {}", l);
    }
    println!("   Logs: {}/run/service.log", dir.display());
    println!("   Press Ctrl+C to stop.");

    tokio::select! {
        _ = router_handle => {},
        _ = tokio::signal::ctrl_c() => {
            println!("Stopping...");
            let _ = unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
        }
    }
    Ok(())
}

fn find_build_artifact(root: &Path, bin_name: &std::ffi::OsStr) -> Result<PathBuf> {
    let local_tgt = root.join("target/release").join(bin_name);
    if local_tgt.exists() {
        return Ok(local_tgt);
    }

    let mut up = root.to_path_buf();
    while up.pop() {
        let ws_tgt = up.join("target/release").join(bin_name);
        if ws_tgt.exists() {
            return Ok(ws_tgt);
        }
        if !up.join("Cargo.toml").exists() {
            break;
        }
    }
    bail!("Could not find compiled binary in target/release.")
}

fn snapshot_dependencies(cell_dir: &Path, deps: &HashMap<String, String>) -> Result<()> {
    let schema_dir = cell_dir.join(".cell-schemas");
    fs::create_dir_all(&schema_dir)?;
    let parent = cell_dir.parent().ok_or_else(|| anyhow!("No parent"))?;

    for (service, path_or_url) in deps {
        // SKIP snapshotting for remote TCP services for now in Phase 1.
        // We assume the schema is already locally available or manual.
        if path_or_url.starts_with("tcp://") {
            continue;
        }

        let sock = parent.join(path_or_url).join("run/cell.sock");
        if sock.exists() {
            if let Ok(mut stream) = UnixStream::connect(&sock) {
                stream.set_read_timeout(Some(Duration::from_millis(500)))?;
                let req = b"__SCHEMA__";
                if stream.write_all(&(req.len() as u32).to_be_bytes()).is_ok()
                    && stream.write_all(req).is_ok()
                {
                    let mut len_buf = [0u8; 4];
                    if stream.read_exact(&mut len_buf).is_ok() {
                        let len = u32::from_be_bytes(len_buf) as usize;
                        let mut buf = vec![0u8; len];
                        if stream.read_exact(&mut buf).is_ok() {
                            let _ = fs::write(schema_dir.join(format!("{}.json", service)), buf);
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn read_manifest(dir: &Path) -> Result<CellManifest> {
    let txt = fs::read_to_string(dir.join("cell.toml"))?;
    toml::from_str(&txt).context("Invalid TOML")
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
    let sock = dir.join("run/cell.sock");

    let mut attempts = 0;
    while attempts < 5 {
        match UnixStream::connect(&sock) {
            Ok(mut stream) => {
                let b = args.as_bytes();
                stream.write_all(&(b.len() as u32).to_be_bytes())?;
                stream.write_all(b)?;

                let mut len_buf = [0u8; 4];
                stream.read_exact(&mut len_buf)?;
                let len = u32::from_be_bytes(len_buf) as usize;

                let mut resp = vec![0u8; len];
                stream.read_exact(&mut resp)?;
                println!("{}", String::from_utf8_lossy(&resp));
                return Ok(());
            }
            Err(_) => {
                std::thread::sleep(Duration::from_millis(100));
                attempts += 1;
            }
        }
    }
    bail!(
        "Connection refused at {}. The cell is not responding.",
        sock.display()
    );
}
