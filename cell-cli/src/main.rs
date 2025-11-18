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
#[command(about = "Cell-native orchestrator (directory-centric MVP)")]
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

// ---------- MAIN ----------

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

// ---------- RUN ----------

fn cmd_run(dir: &Path) -> Result<()> {
    let dir = dir
        .canonicalize()
        .with_context(|| format!("directory not found: {}", dir.display()))?;

    let mf = read_manifest(&dir)?;
    let run_dir = dir.join("run");
    let sock_path = run_dir.join("cell.sock");
    let bin_path = dir.join(&mf.cell.binary);

    fs::create_dir_all(&run_dir)?;

    // This ensures the schemas exist so the macros can read them during compilation.
    if !mf.deps.is_empty() {
        println!("📸 Snapshotting dependencies...");
        if let Err(e) = snapshot_dependencies(&dir, &mf.deps) {
            eprintln!("   ⚠️  Warning: failed to snapshot dependencies: {}", e);
            eprintln!("   (Make sure 'worker' and 'aggregator' are running first!)");
        }
    }

    // Build if binary missing
    if !bin_path.is_file() {
        println!("🔨  Building …");
        build_in_place(&dir, Path::new(&mf.cell.binary))?;
    }

    // Spawn nucleus
    let current_exe = std::env::current_exe()?;
    let log_file = fs::File::create(run_dir.join("nucleus.log"))?;
    let mut cmd = Command::new(current_exe);
    cmd.arg("nucleus")
        .arg(&sock_path)
        .arg(fs::canonicalize(&bin_path)?)
        .current_dir(&dir)
        .stdout(Stdio::null())
        .stderr(Stdio::from(log_file.try_clone()?));

    let child = cmd.spawn().context("spawn nucleus failed")?;
    fs::write(run_dir.join("pid"), child.id().to_string())?;

    // Wait for socket
    for _ in 0..50 {
        if sock_path.exists() {
            println!("✓ Started {} (pid {})", mf.cell.name, child.id());
            // (Removed snapshot logic from here)
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let stderr = fs::read_to_string(run_dir.join("nucleus.log"))?;
    let last = stderr.lines().rev().take(20).collect::<Vec<_>>().join("\n");
    bail!("nucleus failed to create socket:\n{}", last);
}

// ---------- AUTO-SNAPSHOT DEPENDENCIES ----------

fn snapshot_dependencies(cell_dir: &Path, deps: &HashMap<String, String>) -> Result<()> {
    let schema_dir = cell_dir.join(".cell-schemas");
    fs::create_dir_all(&schema_dir)?;

    for (service_name, _version) in deps {
        // Try to fetch schema from running service
        match fetch_schema_from_running_service(service_name) {
            Ok(schema_json) => {
                let hash = blake3::hash(schema_json.as_bytes()).to_hex().to_string();

                let schema_path = schema_dir.join(format!("{}.json", service_name));
                let hash_path = schema_dir.join(format!("{}.hash", service_name));

                // Only update if changed
                let needs_update = match fs::read_to_string(&hash_path) {
                    Ok(existing_hash) => existing_hash.trim() != hash,
                    Err(_) => true,
                };

                if needs_update {
                    fs::write(&schema_path, &schema_json)?;
                    fs::write(&hash_path, &hash)?;
                    println!("   ✓ {} ({}...)", service_name, &hash[..8]);
                } else {
                    println!("   ✓ {} (unchanged)", service_name);
                }
            }
            Err(e) => {
                // Service not running - that's okay, snapshot might exist from before
                let schema_path = schema_dir.join(format!("{}.json", service_name));
                if schema_path.exists() {
                    println!(
                        "   → {} (using cached snapshot, service not running)",
                        service_name
                    );
                } else {
                    eprintln!("   ⚠️  {} not running and no cached snapshot", service_name);
                    eprintln!("       Start it with: cell run <path-to-{}>", service_name);
                }
            }
        }
    }

    Ok(())
}

fn fetch_schema_from_running_service(service_name: &str) -> Result<String> {
    let sock_path = format!("/tmp/cell/sockets/{}.sock", service_name);

    let mut stream = UnixStream::connect(&sock_path)
        .with_context(|| format!("service '{}' not running", service_name))?;

    stream.set_read_timeout(Some(Duration::from_secs(2)))?;

    // Send __SCHEMA__ request
    let req = b"__SCHEMA__";
    stream.write_all(&(req.len() as u32).to_be_bytes())?;
    stream.write_all(req)?;
    stream.flush()?;

    // Read response
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;

    if len > 10 * 1024 * 1024 {
        bail!("schema too large: {} bytes", len);
    }

    let mut schema_bytes = vec![0u8; len];
    stream.read_exact(&mut schema_bytes)?;

    String::from_utf8(schema_bytes).context("invalid UTF-8 in schema")
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

fn cmd_use(dir: &Path, args: &str) -> Result<()> {
    let _mf = read_manifest(dir)?;
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

        let workspace_root = find_workspace_root(root)?;
        let is_workspace_member = workspace_root != root;

        if is_workspace_member {
            let package_name = extract_package_name(root)?;
            println!("   Detected workspace member: {}", package_name);
            println!(
                "   Building from workspace root: {}",
                workspace_root.display()
            );

            run_command(
                Command::new("cargo")
                    .args(&["build", "--release", "-p", &package_name])
                    .current_dir(&workspace_root)
                    .stdout(Stdio::null()),
                &format!("cargo build --release -p {}", package_name),
            )?;

            let name = bin_rel
                .file_name()
                .ok_or_else(|| anyhow!("binary path has no file name"))?;
            let cargo_out = workspace_root.join("target/release").join(name);
            let wanted = root.join(bin_rel);

            if !cargo_out.exists() {
                bail!("cargo succeeded but {} not found", cargo_out.display());
            }
            fs::create_dir_all(wanted.parent().unwrap())?;
            fs::copy(&cargo_out, &wanted)?;
            println!("   Copied {} → {}", cargo_out.display(), wanted.display());
        } else {
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
        }
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

// ---------- WORKSPACE DETECTION ----------

fn find_workspace_root(start: &Path) -> Result<PathBuf> {
    let mut current = start.canonicalize()?;

    loop {
        if let Some(parent) = current.parent() {
            let parent_cargo = parent.join("Cargo.toml");
            if parent_cargo.exists() {
                let content = fs::read_to_string(&parent_cargo)
                    .context("failed to read parent Cargo.toml")?;
                if content.contains("[workspace]") {
                    if is_workspace_member(parent, &start.canonicalize()?)? {
                        return Ok(parent.to_path_buf());
                    }
                }
            }
            current = parent.to_path_buf();
        } else {
            break;
        }
    }

    Ok(start.to_path_buf())
}

fn is_workspace_member(workspace_root: &Path, member_path: &Path) -> Result<bool> {
    let cargo_toml = workspace_root.join("Cargo.toml");
    let content = fs::read_to_string(&cargo_toml)?;

    let cargo: toml::Value = toml::from_str(&content)?;
    if let Some(workspace) = cargo.get("workspace") {
        if let Some(members) = workspace.get("members") {
            if let Some(members_array) = members.as_array() {
                let member_canonical = member_path.canonicalize()?;

                for member in members_array {
                    if let Some(member_str) = member.as_str() {
                        let member_full_path = workspace_root.join(member_str).canonicalize().ok();

                        if member_full_path == Some(member_canonical.clone()) {
                            return Ok(true);
                        }

                        if member_str.contains('*') {
                            let pattern = member_str.replace('*', "");
                            if let Ok(member_parent) = workspace_root
                                .join(pattern.trim_end_matches('/'))
                                .canonicalize()
                            {
                                if member_canonical.starts_with(&member_parent) {
                                    return Ok(true);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(false)
}

fn extract_package_name(root: &Path) -> Result<String> {
    let cargo_toml = root.join("Cargo.toml");
    let content = fs::read_to_string(&cargo_toml)?;
    let cargo: toml::Value = toml::from_str(&content)?;

    cargo
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow!("No package.name found in Cargo.toml"))
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
    Ok(())
}
