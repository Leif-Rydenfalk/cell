use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use regex::Regex;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc; // Added for Golgi Arc optimization
use std::time::SystemTime;
use tokio::net::TcpStream;

// Import from internal lib
use cell_cli::golgi::{Golgi, Target};
use cell_cli::{antigens, nucleus, synapse, vacuole};

#[derive(Parser)]
#[command(name = "membrane")]
#[command(about = "Cellular Infrastructure Node Manager", long_about = None)]
struct Cli {
    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    Mitosis { dir: PathBuf },
}

#[derive(Deserialize, Debug, Clone)]
struct Genome {
    genome: Option<CellTraits>,
    #[serde(default)]
    axons: HashMap<String, String>,
    #[serde(default)]
    junctions: HashMap<String, String>,
    workspace: Option<WorkspaceTraits>,
}

#[derive(Deserialize, Debug, Clone)]
struct WorkspaceTraits {
    members: Vec<String>,
}

#[derive(Deserialize, Debug, Clone)]
struct CellTraits {
    name: String,
    #[serde(default)]
    listen: Option<String>,
    #[serde(default)]
    replicas: Option<u32>,
}

fn sys_log(level: &str, msg: &str) {
    let timestamp = humantime::format_rfc3339_seconds(SystemTime::now());
    eprintln!("[{}] [{}] [MEMBRANE] {}", timestamp, level, msg);
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.action {
        Action::Mitosis { dir } => mitosis(&dir).await,
    }
}

async fn mitosis(dir: &Path) -> Result<()> {
    let dir = dir.canonicalize().context("Invalid directory")?;

    // GENESIS PHASE (Auto-generate schemas from source)
    // This solves the "Protein synthesis failed" chicken-and-egg error.
    sys_log("INFO", "Running Genesis phase (Schema extraction)...");
    if let Err(e) = run_genesis(&dir) {
        sys_log("WARN", &format!("Genesis incomplete: {}", e));
        // We don't abort, because maybe the user provided them manually.
    }

    let genome_path = dir.join("genome.toml");

    sys_log(
        "INFO",
        &format!("Reading genome from {}", genome_path.display()),
    );

    let txt = std::fs::read_to_string(&genome_path).context("Missing genome.toml")?;
    let dna: Genome = toml::from_str(&txt).context("Corrupt DNA (Invalid TOML)")?;

    // --- WORKSPACE MODE (ORCHESTRATOR) ---
    if let Some(ws) = dna.workspace {
        sys_log("INFO", "System detected. Commencing Multi-Cell Mitosis...");
        let self_exe = std::env::current_exe()?;

        let mut children = Vec::new();

        for member in ws.members {
            let member_path = dir.join(&member);
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;

            let mut cmd = tokio::process::Command::new(&self_exe);
            cmd.arg("mitosis").arg(member_path);
            cmd.kill_on_drop(true);
            cmd.stdout(Stdio::inherit());
            cmd.stderr(Stdio::inherit());

            let child = cmd.spawn().context("Failed to spawn member")?;
            children.push(child);
        }

        sys_log("INFO", "System Running. Press Ctrl+C to shutdown.");
        tokio::signal::ctrl_c().await?;
        return Ok(());
    }

    // --- CELL MODE ---
    let traits = dna.genome.context("Invalid genome")?;

    // 1. Snapshot Remote Dependencies
    if !dna.axons.is_empty() {
        snapshot_genomes(&dir, &dna.axons).await?;
    }

    // 2. Build & Locate Binary
    sys_log(
        "INFO",
        &format!("Synthesizing proteins for {}...", traits.name),
    );

    let output = Command::new("cargo")
        .args(&["build", "--release", "--message-format=json"])
        .current_dir(&dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output()?;

    if !output.status.success() {
        anyhow::bail!("Protein synthesis failed.");
    }

    let reader = std::io::BufReader::new(output.stdout.as_slice());
    let mut bin_path: Option<PathBuf> = None;

    use std::io::BufRead;
    for line in reader.lines() {
        if let Ok(l) = line {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&l) {
                if val["reason"] == "compiler-artifact" && val["target"]["name"] == traits.name {
                    if let Some(executable) = val["executable"].as_str() {
                        bin_path = Some(PathBuf::from(executable));
                    }
                }
            }
        }
    }
    let bin_path = bin_path.ok_or_else(|| anyhow!("Could not locate binary"))?;

    let run_dir = dir.join("run");
    if run_dir.exists() {
        std::fs::remove_dir_all(&run_dir)?;
    }
    std::fs::create_dir_all(&run_dir)?;

    let mut routes = HashMap::new();

    // --- COLONY / REPLICA LOGIC ---
    let replicas = traits.replicas.unwrap_or(1);
    let mut child_guards = Vec::new();
    let golgi_sock_path = run_dir.join("golgi.sock");

    if replicas > 1 {
        sys_log("INFO", &format!("Spawning Colony: {} workers.", replicas));

        let socket_dir = run_dir.join("sockets");
        std::fs::create_dir_all(&socket_dir)?;

        // Setup Vacuole (Shared Logging)
        let log_path = run_dir.join("service.log");
        let vacuole = vacuole::Vacuole::new(log_path).await?;

        let mut worker_sockets = Vec::new();

        for i in 0..replicas {
            // Use subdirectory for isolation of socket (prevents name collisions)
            let worker_dir = run_dir.join("workers").join(i.to_string());
            std::fs::create_dir_all(&worker_dir)?;
            let sock_path = worker_dir.join("cell.sock");

            worker_sockets.push(sock_path.clone());

            // LogStrategy::Piped -> streams back to parent -> Vacuole
            let mut guard = nucleus::activate(
                &sock_path,
                nucleus::LogStrategy::Piped,
                &bin_path,
                &golgi_sock_path,
            )?;

            // Attach pipes to Vacuole
            let (out, err) = guard.take_pipes();
            vacuole.attach(format!("w-{}", i), out, err);

            child_guards.push(guard);
        }

        routes.insert(
            traits.name.clone(),
            Target::LocalColony(Arc::new(worker_sockets)), // Wrap in Arc for Golgi
        );
    } else {
        // Single Cell Mode (Direct File Logging)
        let cell_sock = run_dir.join("cell.sock");
        let log_path = run_dir.join("service.log");

        let guard = nucleus::activate(
            &cell_sock,
            nucleus::LogStrategy::File(log_path),
            &bin_path,
            &golgi_sock_path,
        )?;
        child_guards.push(guard);

        routes.insert(traits.name.clone(), Target::GapJunction(cell_sock));
    }

    // --- LOCAL JUNCTIONS ---
    for (name, path) in dna.junctions {
        routes.insert(
            name,
            Target::GapJunction(dir.join(path).join("run/cell.sock")),
        );
    }

    // --- REMOTE AXONS (STATIC ROUTES) ---
    for (name, addr) in dna.axons {
        let clean = addr.replace("axon://", "");
        routes.insert(
            name,
            Target::AxonCluster(vec![cell_cli::golgi::AxonTerminal {
                id: "static".into(),
                addr: clean,
                rtt: std::time::Duration::from_secs(1),
                last_seen: std::time::Instant::now(),
            }]),
        );
    }

    // Initialize Golgi
    let golgi = Golgi::new(traits.name.clone(), &run_dir, traits.listen.clone(), routes)?;

    sys_log(
        "INFO",
        &format!("Cell '{}' (or Colony) is operational.", traits.name),
    );

    tokio::select! {
        res = golgi.run() => {
            if let Err(e) = res {
                sys_log("CRITICAL", &format!("Golgi failure: {}", e));
            }
        },
        _ = tokio::signal::ctrl_c() => sys_log("INFO", "Apoptosis triggered..."),
    }

    Ok(())
}

async fn snapshot_genomes(root: &Path, axons: &HashMap<String, String>) -> Result<()> {
    let schema_dir = root.join(".cell-genomes");
    std::fs::create_dir_all(&schema_dir)?;
    let temp_id_path = root.join("run/temp_builder_identity");
    let identity = antigens::Antigens::load_or_create(temp_id_path)?;

    for (name, addr) in axons {
        let clean_addr = addr.replace("axon://", "");
        sys_log(
            "INFO",
            &format!("Fetching schema from {} ({})", name, clean_addr),
        );

        let start = std::time::Instant::now();
        let mut connected = false;

        while start.elapsed() < std::time::Duration::from_secs(10) {
            if let Ok(stream) = TcpStream::connect(&clean_addr).await {
                if let Ok((mut secure, _)) =
                    synapse::connect_secure(stream, &identity.keypair, true).await
                {
                    let mut buf = vec![0u8; 4096];
                    // Connect Frame
                    let mut payload = vec![0x01];
                    payload.extend(&(name.len() as u32).to_be_bytes());
                    payload.extend(name.as_bytes());
                    let len = secure.state.write_message(&payload, &mut buf).unwrap();
                    synapse::write_frame(&mut secure.inner, &buf[..len]).await?;

                    // Read ACK
                    let frame = synapse::read_frame(&mut secure.inner).await?;
                    let len = secure.state.read_message(&frame, &mut buf)?;
                    if len > 0 && buf[0] == 0x00 {
                        // Fetch Genome
                        let req = b"__GENOME__";
                        let mut v = (req.len() as u32).to_be_bytes().to_vec();
                        v.extend_from_slice(req);
                        let len = secure.state.write_message(&v, &mut buf).unwrap();
                        synapse::write_frame(&mut secure.inner, &buf[..len]).await?;

                        let frame = synapse::read_frame(&mut secure.inner).await?;
                        let len = secure.state.read_message(&frame, &mut buf)?;
                        if len >= 4 {
                            let jlen = u32::from_be_bytes(buf[0..4].try_into().unwrap()) as usize;
                            if len >= 4 + jlen {
                                let json = &buf[4..4 + jlen];
                                std::fs::write(schema_dir.join(format!("{}.json", name)), json)?;
                                connected = true;
                                break;
                            }
                        }
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        if !connected {
            sys_log("WARN", &format!("Could not fetch schema for {}", name));
        }
    }
    Ok(())
}

/// Recursively scans the project's `src` folder for `signal_receptor!` definitions
/// and generates the `.cell-genomes/{name}.json` files required for compilation.
fn run_genesis(root: &Path) -> Result<()> {
    let src_dir = root.join("src");
    let schema_dir = root.join(".cell-genomes");

    // If src doesn't exist (e.g. workspace root), we skip silently
    if !src_dir.exists() {
        return Ok(());
    }

    std::fs::create_dir_all(&schema_dir)?;

    // 1. Compile Regex once
    // Matches: signal_receptor! { name: foo, input: Bar ... output: Baz ... }
    // (?s) enables "dot matches newline" to handle multi-line macro invocations.
    let re = Regex::new(
        r"(?s)signal_receptor!\s*\{\s*name:\s*([a-zA-Z0-9_]+)\s*,\s*input:\s*([a-zA-Z0-9_]+).*?output:\s*([a-zA-Z0-9_]+)",
    )?;

    // 2. Recursive Walk
    visit_dirs(&src_dir, &|entry| {
        process_file(entry.path(), &schema_dir, &re)
    })?;

    Ok(())
}

/// Helper to recursively walk directories
fn visit_dirs(dir: &Path, cb: &dyn Fn(&fs::DirEntry) -> Result<()>) -> io::Result<()> {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                visit_dirs(&path, cb)?;
            } else {
                cb(&entry).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
            }
        }
    }
    Ok(())
}

/// Analyzes a single file
fn process_file(path: PathBuf, schema_dir: &Path, re: &Regex) -> Result<()> {
    // Only check Rust files
    if path.extension().map_or(false, |ext| ext == "rs") {
        let content = fs::read_to_string(&path)?;

        // Strip comments to avoid parsing commented-out macros
        let clean_content = strip_comments(&content);

        // Iterate over all matches in the file (a file might define multiple receptors)
        for cap in re.captures_iter(&clean_content) {
            let cell_name = &cap[1];
            let input_type = &cap[2];
            let output_type = &cap[3];

            let json = format!(
                r#"{{ "input": "{}", "output": "{}" }}"#,
                input_type, output_type
            );

            let dest = schema_dir.join(format!("{}.json", cell_name));
            fs::write(&dest, json)?;

            sys_log(
                "INFO",
                &format!("Genesis: Synthesized schema for '{}'", cell_name),
            );
        }
    }
    Ok(())
}

/// Rudimentary comment stripper to prevent false positives.
/// Removes // line comments and /* block comments */.
fn strip_comments(code: &str) -> String {
    let mut result = String::with_capacity(code.len());
    let mut chars = code.chars().peekable();
    let mut in_string = false;

    while let Some(c) = chars.next() {
        if in_string {
            result.push(c);
            if c == '"' {
                in_string = false;
            }
            // Handle escaped quotes inside string
            if c == '\\' {
                if let Some(next) = chars.next() {
                    result.push(next);
                }
            }
        } else {
            if c == '"' {
                in_string = true;
                result.push(c);
            } else if c == '/' {
                match chars.peek() {
                    Some('/') => {
                        // Line comment: Skip until newline
                        chars.next(); // consume 2nd /
                        while let Some(n) = chars.next() {
                            if n == '\n' {
                                result.push('\n');
                                break;
                            }
                        }
                    }
                    Some('*') => {
                        // Block comment: Skip until */
                        chars.next(); // consume *
                        while let Some(n) = chars.next() {
                            if n == '*' {
                                if let Some('/') = chars.peek() {
                                    chars.next(); // consume /
                                    break;
                                }
                            }
                        }
                    }
                    _ => result.push(c), // Just a division sign
                }
            } else {
                result.push(c);
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_strip_comments() {
        let code = r#"
            // This is a comment
            signal_receptor! { name: valid, input: A, output: B }
            /* Block comment 
               signal_receptor! { name: invalid, input: X, output: Y }
            */
            let s = "string with // comment chars";
        "#;

        let cleaned = strip_comments(code);
        assert!(cleaned.contains("name: valid"));
        assert!(!cleaned.contains("name: invalid"));
        assert!(cleaned.contains("string with // comment chars"));
    }

    #[test]
    fn test_genesis_discovery() -> Result<()> {
        let dir = tempdir()?;
        let src = dir.path().join("src");
        fs::create_dir(&src)?;

        // 1. Standard formatting
        fs::write(
            src.join("main.rs"),
            r#"
            signal_receptor! {
                name: standard,
                input: Request,
                output: Response
            }
        "#,
        )?;

        // 2. Minified/One-liner
        fs::write(
            src.join("compact.rs"),
            r#"signal_receptor!{name:compact,input:In,output:Out}"#,
        )?;

        // 3. Commented out (Should NOT be found)
        fs::write(
            src.join("ignored.rs"),
            r#"
            // signal_receptor! { name: ghost, input: A, output: B }
        "#,
        )?;

        // 4. Deeply nested file
        let deep = src.join("deep/nested");
        fs::create_dir_all(&deep)?;
        fs::write(
            deep.join("mod.rs"),
            r#"
            pub mod inner {
                signal_receptor! {
                    name: nested_cell,
                    input: DeepIn,
                    output: DeepOut
                }
            }
        "#,
        )?;

        // Run Genesis
        run_genesis(dir.path())?;

        let schema_dir = dir.path().join(".cell-genomes");

        // Assertions
        assert!(schema_dir.join("standard.json").exists());
        assert!(schema_dir.join("compact.json").exists());
        assert!(schema_dir.join("nested_cell.json").exists());
        assert!(!schema_dir.join("ghost.json").exists());

        // Verify Content
        let standard_json = fs::read_to_string(schema_dir.join("standard.json"))?;
        assert_eq!(
            standard_json,
            r#"{ "input": "Request", "output": "Response" }"#
        );

        Ok(())
    }
}
