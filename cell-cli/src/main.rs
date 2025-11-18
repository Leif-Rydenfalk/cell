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

mod nucleus;

#[derive(Parser)]
#[command(name = "cell")]
#[command(about = "Cell-native orchestrator")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run (build if needed) and start the cell inside its own directory
    Run { dir: PathBuf },
    /// Stop the cell (send SIGTERM, clean run/, keep rest)
    Stop { dir: PathBuf },
    /// Use a cell (invoke a function via Unix socket)
    Use {
        dir: PathBuf,
        #[arg(name = "FN_NAME")]
        fn_name: String,
        args: String,
    },
    /// Garbage-collect unused cells (no-op in this version)
    Gc,
    /// Internal: wrap a cell binary (used by cell run)
    Nucleus { socket: PathBuf, binary: PathBuf },
}

// ---------- CONFIGURATION DATA ----------

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
    idle_timeout: Option<u64>,
    auto_cleanup: Option<bool>,
}

#[derive(Deserialize, Debug, Default)]
struct Artefact {
    artefact_type: Option<String>,
}

fn default_true() -> bool {
    true
}

// ---------- MAIN ENTRY POINT ----------

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Run { dir } => cmd_run(&dir),
        Commands::Stop { dir } => cmd_stop(&dir),
        Commands::Use {
            dir,
            fn_name: _,
            args,
        } => cmd_use(&dir, &args),
        Commands::Gc => cmd_gc(),
        Commands::Nucleus { socket, binary } => nucleus::run_nucleus(&socket, &binary),
    }
}

// ---------- COMMAND: RUN ----------

fn cmd_run(dir: &Path) -> Result<()> {
    let dir = dir
        .canonicalize()
        .with_context(|| format!("Directory not found: {}", dir.display()))?;

    let mf = read_manifest(&dir)?;
    let run_dir = dir.join("run");
    let sock_path = run_dir.join("cell.sock");
    let bin_path = dir.join(&mf.cell.binary);

    fs::create_dir_all(&run_dir).context("Failed to create run directory")?;

    // 1. SNAPSHOT DEPENDENCIES (CRITICAL: MUST HAPPEN BEFORE BUILD)
    // This ensures the compiler macros can find the .cell-schemas JSON files.
    if !mf.deps.is_empty() {
        println!("📸 Snapshotting dependencies...");
        if let Err(e) = snapshot_dependencies(&dir, &mf.deps) {
            eprintln!("   ❌ Snapshot failed.");
            eprintln!("      Reason: {:#}", e);
            eprintln!("      (Build may fail if macros cannot find schemas)");
        }
    }

    // 2. BUILD
    // We only build if the binary is missing.
    // In a production CI/CD, you might want a --force-rebuild flag.
    if !bin_path.is_file() {
        println!("🔨  Building '{}'...", mf.cell.name);
        build_in_place(&dir, Path::new(&mf.cell.binary)).context("Build failed")?;
    }

    // 3. SPAWN NUCLEUS
    // The nucleus acts as the supervisor/parent process.
    let current_exe = std::env::current_exe().context("Failed to get current executable path")?;
    let log_file =
        fs::File::create(run_dir.join("nucleus.log")).context("Failed to create nucleus.log")?;

    let mut cmd = Command::new(current_exe);
    cmd.arg("nucleus")
        .arg(&sock_path)
        .arg(fs::canonicalize(&bin_path).context("Binary disappeared after build")?)
        .current_dir(&dir)
        .stdout(Stdio::null())
        .stderr(Stdio::from(
            log_file.try_clone().context("Failed to clone log handle")?,
        ));

    // Inject Dependency Locations as Environment Variables.
    // This allows the SDK macros to find peers at runtime without hardcoding paths.
    for (dep_name, _) in &mf.deps {
        // Cell-centric logic: Peers are siblings in the parent directory.
        let parent = dir
            .parent()
            .ok_or_else(|| anyhow!("Cell root has no parent"))?;
        let dep_sock = parent.join(dep_name).join("run/cell.sock");

        // We pass the path even if it doesn't exist yet (it might start later),
        // but canonicalize requires existence. So we pass absolute path if possible.
        let env_key = format!("CELL_DEP_{}_SOCK", dep_name.to_uppercase());

        // Try to canonicalize, otherwise construct absolute path manually if possible
        if let Ok(abs) = dep_sock.canonicalize() {
            cmd.env(&env_key, abs);
        } else {
            // Best effort absolute path if service isn't running yet
            cmd.env(&env_key, dep_sock);
        }
    }

    let child = cmd.spawn().context("Failed to spawn nucleus")?;
    fs::write(run_dir.join("pid"), child.id().to_string())?;

    // 4. WAIT FOR SOCKET
    println!("🚀 Spawning {} (pid {})...", mf.cell.name, child.id());

    let timeout = Duration::from_secs(5);
    let start = std::time::Instant::now();

    while start.elapsed() < timeout {
        if sock_path.exists() {
            // Validate connection
            if UnixStream::connect(&sock_path).is_ok() {
                println!("✓ Started {} successfully.", mf.cell.name);
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    // If we timed out, read the log to show user what happened
    let stderr = fs::read_to_string(run_dir.join("nucleus.log"))
        .unwrap_or_else(|_| "Could not read nucleus.log".into());
    let last_lines = stderr.lines().rev().take(10).collect::<Vec<_>>().join("\n");

    bail!(
        "Timed out waiting for socket at {}.\nNucleus Log:\n{}",
        sock_path.display(),
        last_lines
    );
}

// ---------- DEPENDENCY SNAPSHOTTING ----------

fn snapshot_dependencies(cell_dir: &Path, deps: &HashMap<String, String>) -> Result<()> {
    let schema_dir = cell_dir.join(".cell-schemas");
    fs::create_dir_all(&schema_dir).context("Failed to create .cell-schemas dir")?;

    for (service_name, _version) in deps {
        // Logic: Assume workspace layout.
        // Cell:  /workspace/my-cell
        // Dep:   /workspace/dep-cell/run/cell.sock
        let parent = cell_dir
            .parent()
            .ok_or_else(|| anyhow!("Cell has no parent directory"))?;
        let sibling_sock = parent.join(service_name).join("run/cell.sock");

        print!("   → Checking '{}' ... ", service_name);
        let _ = std::io::stdout().flush();

        match fetch_schema(&sibling_path_to_connect(&sibling_sock)) {
            Ok(schema_json) => {
                let hash = blake3::hash(schema_json.as_bytes()).to_hex().to_string();

                let schema_path = schema_dir.join(format!("{}.json", service_name));
                let hash_path = schema_dir.join(format!("{}.hash", service_name));

                let needs_update = match fs::read_to_string(&hash_path) {
                    Ok(existing_hash) => existing_hash.trim() != hash,
                    Err(_) => true,
                };

                if needs_update {
                    fs::write(&schema_path, &schema_json)?;
                    fs::write(&hash_path, &hash)?;
                    println!("✓ UPDATED");
                } else {
                    println!("✓ OK (Cached)");
                }
            }
            Err(e) => {
                // Fallback: Check if we have a cached snapshot to allow offline builds
                let schema_path = schema_dir.join(format!("{}.json", service_name));
                if schema_path.exists() {
                    println!("⚠️  OFFLINE (Using cached snapshot)");
                } else {
                    println!("❌ FAILED");
                    bail!(
                        "Dependency '{}' is unreachable at {}\nError: {}",
                        service_name,
                        sibling_sock.display(),
                        e
                    );
                }
            }
        }
    }
    Ok(())
}

/// Helper to handle path ownership for connect
fn sibling_path_to_connect(p: &Path) -> PathBuf {
    p.to_path_buf()
}

fn fetch_schema(sock_path: &Path) -> Result<String> {
    let mut stream = UnixStream::connect(sock_path).with_context(|| "Connection refused")?;

    stream.set_read_timeout(Some(Duration::from_secs(2)))?;

    // Protocol: Send length-prefixed request
    let req = b"__SCHEMA__";
    stream.write_all(&(req.len() as u32).to_be_bytes())?;
    stream.write_all(req)?;
    stream.flush()?;

    // Read length-prefixed response
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;

    if len > 10 * 1024 * 1024 {
        bail!("Schema too large ({} bytes)", len);
    }

    let mut schema_bytes = vec![0u8; len];
    stream.read_exact(&mut schema_bytes)?;

    String::from_utf8(schema_bytes).context("Invalid UTF-8 in schema")
}

// ---------- COMMAND: STOP ----------

fn cmd_stop(dir: &Path) -> Result<()> {
    let pid_file = dir.join("run/pid");
    if !pid_file.exists() {
        bail!("Not running (no pid file found)");
    }

    let pid_str = fs::read_to_string(&pid_file)?;
    let pid = pid_str
        .trim()
        .parse::<i32>()
        .context("Invalid PID file content")?;

    unsafe { libc::kill(pid, libc::SIGTERM) };

    // Cleanup
    let _ = fs::remove_file(pid_file);

    // Try to remove socket too
    let sock = dir.join("run/cell.sock");
    if sock.exists() {
        let _ = fs::remove_file(sock);
    }

    println!("✓ Stopped (pid {})", pid);
    Ok(())
}

// ---------- COMMAND: USE ----------

fn cmd_use(dir: &Path, args: &str) -> Result<()> {
    let sock = dir.join("run/cell.sock");
    if !sock.exists() {
        bail!("Socket not found at {}", sock.display());
    }

    let req_json = if args == "-" {
        std::io::read_to_string(std::io::stdin())?
    } else {
        args.into()
    };

    // Simple one-off RPC client
    let mut stream = UnixStream::connect(&sock)
        .with_context(|| format!("Failed to connect to {}", sock.display()))?;

    let req_bytes = req_json.as_bytes();
    stream.write_all(&(req_bytes.len() as u32).to_be_bytes())?;
    stream.write_all(req_bytes)?;
    stream.flush()?;

    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;

    let mut resp_buf = vec![0u8; len];
    stream.read_exact(&mut resp_buf)?;

    println!("{}", String::from_utf8_lossy(&resp_buf));
    Ok(())
}

// ---------- BUILD SYSTEM ----------

fn build_in_place(root: &Path, bin_rel: &Path) -> Result<()> {
    if root.join("Cargo.toml").exists() {
        build_cargo(root, bin_rel)
    } else if root.join("Makefile").exists() {
        println!("   Building with Makefile …");
        run_command(Command::new("make").current_dir(root), "make")
    } else if root.join("build.sh").exists() {
        println!("   Running build.sh …");
        run_command(Command::new("./build.sh").current_dir(root), "./build.sh")
    } else {
        bail!("No build recipe found (Cargo.toml, Makefile, or build.sh)")
    }
}

fn build_cargo(root: &Path, bin_rel: &Path) -> Result<()> {
    println!("   Running cargo build --release...");

    // Heuristic: Detect if we are inside a workspace
    let workspace_root = find_workspace_root(root)?;
    let is_member = workspace_root != root;

    // If it's a member, we should ideally run from workspace root with -p <pkg>
    // This prevents locking contention on target/ dir
    let mut cmd = Command::new("cargo");
    cmd.arg("build").arg("--release");

    if is_member {
        let pkg_name = extract_package_name(root)?;
        cmd.arg("-p").arg(&pkg_name);
        cmd.current_dir(&workspace_root);
    } else {
        cmd.current_dir(root);
    }

    cmd.stdout(Stdio::null()); // Silence standard output

    let status = cmd.status().context("Failed to execute cargo")?;
    if !status.success() {
        bail!("Cargo build failed");
    }

    // Locate the artifact.
    // It could be in <ws_root>/target/release or <root>/target/release
    let bin_name = bin_rel
        .file_name()
        .ok_or_else(|| anyhow!("Invalid binary name"))?;

    let possible_paths = vec![
        root.join("target/release").join(bin_name),
        workspace_root.join("target/release").join(bin_name),
    ];

    let src = possible_paths.iter().find(|p| p.exists()).ok_or_else(|| {
        anyhow!(
            "Cargo succeeded, but binary not found. Checked: {:?}",
            possible_paths
        )
    })?;

    let dst = root.join(bin_rel);
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::copy(src, &dst).context("Failed to copy binary to destination")?;
    println!("   Artifact ready: {}", dst.display());
    Ok(())
}

// ---------- UTILS ----------

fn find_workspace_root(start: &Path) -> Result<PathBuf> {
    let mut current = start.canonicalize()?;
    loop {
        if let Some(parent) = current.parent() {
            let cargo = parent.join("Cargo.toml");
            if cargo.exists() {
                let content = fs::read_to_string(&cargo).unwrap_or_default();
                if content.contains("[workspace]") {
                    return Ok(parent.to_path_buf());
                }
            }
            current = parent.to_path_buf();
        } else {
            break;
        }
    }
    Ok(start.to_path_buf())
}

fn extract_package_name(root: &Path) -> Result<String> {
    let content = fs::read_to_string(root.join("Cargo.toml"))?;
    let cargo: toml::Value = toml::from_str(&content)?;
    cargo
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow!("Could not determine package name"))
}

fn read_manifest(dir: &Path) -> Result<CellManifest> {
    let txt = fs::read_to_string(dir.join("cell.toml"))
        .with_context(|| format!("Could not read cell.toml in {}", dir.display()))?;
    toml::from_str(&txt).context("Invalid TOML in cell.toml")
}

fn run_command(cmd: &mut Command, desc: &str) -> Result<()> {
    let status = cmd
        .status()
        .with_context(|| format!("Failed to start: {}", desc))?;
    if !status.success() {
        bail!("{} failed with exit code: {}", desc, status);
    }
    Ok(())
}

fn cmd_gc() -> Result<()> {
    println!("Garbage collection not implemented in MVP.");
    Ok(())
}
