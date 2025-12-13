// cells/hypervisor/src/main.rs
// SPDX-License-Identifier: MIT
// The Daemon: System Hypervisor and Process Manager

mod capsid;

use capsid::Capsid;
use cell_sdk::cell_remote;
use cell_model::protocol::{MitosisRequest, MitosisResponse, MitosisSignal, MitosisControl, TestEvent};
use cell_model::config::CellInitConfig;
use cell_transport::GapJunction;
use anyhow::{Context, Result, anyhow};
use std::path::PathBuf;
use tokio::net::{UnixListener, UnixStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader, AsyncBufReadExt};
use tracing::{info, error};
use cell_model::rkyv::Deserialize;

// Remote interface to Builder
cell_remote!(Builder = "builder");

#[cell_sdk::service]
struct HypervisorService;

#[cell_sdk::handler]
impl HypervisorService {
    async fn spawn(&self, cell_name: String, config: Option<CellInitConfig>) -> Result<()> {
        let _ = cell_name;
        let _ = config;
        Ok(())
    }

    async fn hot_swap(&self, cell_name: String, new_binary: String) -> Result<bool> {
        Ok(true)
    }
}

pub struct Hypervisor {
    system_socket_dir: PathBuf,
    daemon_socket_path: PathBuf,
}

impl Hypervisor {
    pub async fn ignite() -> Result<()> {
        let mut junction = unsafe { GapJunction::open_daughter().expect("Failed to open Gap Junction") };
        junction.signal(MitosisSignal::Prophase)?;
        junction.signal(MitosisSignal::RequestIdentity)?;
        let control = junction.wait_for_control()?;
        let _identity = match control {
            MitosisControl::InjectIdentity(c) => c,
            MitosisControl::Terminate => return Err(anyhow!("Terminated by System")),
        };

        let home = dirs::home_dir().expect("No HOME");
        let system_socket_dir = if let Ok(p) = std::env::var("CELL_SOCKET_DIR") {
            PathBuf::from(p)
        } else {
            home.join(".cell/runtime/system")
        };

        tokio::fs::create_dir_all(&system_socket_dir).await?;
        let daemon_socket_path = system_socket_dir.join("mitosis.sock");

        if daemon_socket_path.exists() { tokio::fs::remove_file(&daemon_socket_path).await?; }
        let listener = UnixListener::bind(&daemon_socket_path)?;

        junction.signal(MitosisSignal::Prometaphase { 
            socket_path: daemon_socket_path.to_string_lossy().to_string() 
        })?;

        info!("[Hypervisor] Kernel Active. Listening on {:?}", daemon_socket_path);

        let hv = Self { 
            system_socket_dir: system_socket_dir.clone(), 
            daemon_socket_path: daemon_socket_path.clone()
        };

        hv.bootstrap_kernel_cell("nucleus").await?;
        hv.bootstrap_kernel_cell("axon").await?;
        hv.bootstrap_kernel_cell("builder").await?;
        
        junction.signal(MitosisSignal::Cytokinesis)?;
        drop(junction);

        let hv_arc = std::sync::Arc::new(hv);
        
        loop {
            if let Ok((stream, _)) = listener.accept().await {
                let r_inner = hv_arc.clone();
                tokio::spawn(async move {
                    if let Err(e) = r_inner.handle_request(stream).await {
                        error!("[Hypervisor] Request Error: {}", e);
                    }
                });
            }
        }
    }

    async fn bootstrap_kernel_cell(&self, name: &str) -> Result<()> {
        use std::process::Command;
        use cell_transport::gap_junction::spawn_with_gap_junction;
        
        let socket = self.system_socket_dir.join(format!("{}.sock", name));
        
        if socket.exists() {
            if tokio::net::UnixStream::connect(&socket).await.is_ok() {
                info!("[Hypervisor] {} is already running", name);
                return Ok(());
            }
            tokio::fs::remove_file(&socket).await.ok();
        }

        info!("[Hypervisor] Bootstrapping {}...", name);

        let mut cmd = Command::new("cargo");
        cmd.arg("run").arg("--release").arg("-p").arg(name);
        
        if let Ok(s) = std::env::var("CELL_SOCKET_DIR") { cmd.env("CELL_SOCKET_DIR", s); }
        if let Ok(r) = std::env::var("CELL_REGISTRY_DIR") { cmd.env("CELL_REGISTRY_DIR", r); }
        if let Ok(h) = std::env::var("HOME") { cmd.env("HOME", h); }
        if let Ok(t) = std::env::var("CARGO_TARGET_DIR") { cmd.env("CARGO_TARGET_DIR", t); }
        cmd.env("CELL_NODE_ID", "0");

        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::inherit());

        let (_child, mut junction) = spawn_with_gap_junction(cmd)?;

        let config = CellInitConfig {
            node_id: 0,
            cell_name: name.to_string(),
            peers: vec![],
            socket_path: socket.to_string_lossy().to_string(),
            organism: "system".to_string(),
        };

        loop {
            match junction.wait_for_signal()? {
                MitosisSignal::RequestIdentity => {
                    junction.send_control(MitosisControl::InjectIdentity(config.clone()))?;
                }
                MitosisSignal::Cytokinesis => {
                    info!("[Hypervisor] {} online.", name);
                    break;
                }
                MitosisSignal::Apoptosis { reason } => anyhow::bail!("{} died: {}", name, reason),
                MitosisSignal::Necrosis => anyhow::bail!("{} panicked", name),
                _ => {}
            }
        }
        Ok(())
    }

    async fn handle_request(&self, mut stream: UnixStream) -> Result<()> {
        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).await?;
        let len = u32::from_le_bytes(len_buf) as usize;
        let mut buf = vec![0u8; len];
        stream.read_exact(&mut buf).await?;

        let req = cell_model::rkyv::check_archived_root::<MitosisRequest>(&buf)
            .map_err(|e| anyhow::anyhow!("Protocol Violation: {}", e))?;

        match req {
            cell_model::protocol::ArchivedMitosisRequest::Spawn { cell_name, config } => {
                let name = cell_name.to_string();
                
                let final_config = if let cell_model::rkyv::option::ArchivedOption::Some(c) = config {
                    c.deserialize(&mut cell_model::rkyv::Infallible).unwrap()
                } else {
                    let socket_path = self.system_socket_dir.join(format!("{}.sock", name));
                    CellInitConfig {
                        node_id: rand::random(),
                        cell_name: name.clone(),
                        peers: vec![],
                        socket_path: socket_path.to_string_lossy().to_string(),
                        organism: "system".to_string(),
                    }
                };

                match self.perform_spawn(&name, &final_config).await {
                    Ok(_) => {
                        let resp = MitosisResponse::Ok { socket_path: final_config.socket_path };
                        self.send_resp(&mut stream, resp).await?;
                    }
                    Err(e) => {
                        self.send_resp(&mut stream, MitosisResponse::Denied { reason: e.to_string() }).await?;
                    }
                }
            }
            cell_model::protocol::ArchivedMitosisRequest::Test { target_cell, filter } => {
                let target = target_cell.to_string();
                let _filter = filter.as_ref().map(|s| s.to_string());
                
                self.perform_test(target, _filter, &mut stream).await?;
            }
        }
        Ok(())
    }

    async fn perform_spawn(&self, cell_name: &str, config: &CellInitConfig) -> Result<()> {
        let mut builder = Builder::Client::connect().await
            .context("Hypervisor cannot reach Builder")?;
            
        let build_res = builder.build(cell_name.to_string(), Builder::BuildMode::Standard).await
            .context("Build failed")?;

        let binary_path = PathBuf::from(build_res.binary_path);
        let socket_path = PathBuf::from(&config.socket_path);
        let runtime_dir = socket_path.parent().unwrap();
        tokio::fs::create_dir_all(runtime_dir).await?;

        Capsid::spawn(&binary_path, runtime_dir, &self.daemon_socket_path, &[], config, false)?;
        Ok(())
    }

    async fn perform_test(&self, target: String, filter: Option<String>, stream: &mut UnixStream) -> Result<()> {
        let mut builder = Builder::Client::connect().await
            .context("Hypervisor cannot reach Builder")?;

        self.send_event(stream, TestEvent::Log(format!("Building tests for '{}'...", target))).await?;

        // 1. Build
        let build_res = match builder.build(target.clone(), Builder::BuildMode::Test).await {
            Ok(r) => r,
            Err(e) => {
                self.send_event(stream, TestEvent::Error(format!("Build failed: {}", e))).await?;
                return Ok(());
            }
        };

        let binary_path = PathBuf::from(build_res.binary_path);
        
        // 2. Config for ephemeral test cell
        let socket_dir = self.system_socket_dir.join("tests");
        tokio::fs::create_dir_all(&socket_dir).await?;
        
        let config = CellInitConfig {
            node_id: 999,
            cell_name: format!("{}-test", target),
            peers: vec![],
            socket_path: socket_dir.join(format!("{}-test.sock", target)).to_string_lossy().to_string(),
            organism: "test".to_string(),
        };

        // 3. Spawn with Pipe
        let mut args = vec![];
        let filter_val;
        if let Some(f) = filter {
            filter_val = f;
            args.push(&filter_val as &str); // Rust test harness args
        }

        let mut child = match Capsid::spawn(&binary_path, &socket_dir, &self.daemon_socket_path, &args, &config, true) {
            Ok(c) => c,
            Err(e) => {
                self.send_event(stream, TestEvent::Error(format!("Spawn failed: {}", e))).await?;
                return Ok(());
            }
        };

        // 4. Stream Output
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        
        // Merge streams (simplified) or handle separate? 
        // Rust test harness prints to stdout usually.
        let mut reader = BufReader::new(stdout).lines();
        
        let mut passed = 0;
        let mut failed = 0;
        let mut total = 0;

        self.send_event(stream, TestEvent::CaseStarted(target.clone())).await?;

        while let Ok(Some(line)) = reader.next_line().await {
            self.send_event(stream, TestEvent::Log(line.clone())).await?;
            
            // Parse summary
            // "test result: ok. 5 passed; 0 failed; ..."
            if line.trim().starts_with("test result:") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if let (Some(p), Some(f)) = (parts.get(3), parts.get(5)) {
                    passed = p.parse().unwrap_or(0);
                    failed = f.parse().unwrap_or(0);
                    total = passed + failed;
                }
            }
        }

        // Wait for exit
        let status = child.wait().await?;
        
        self.send_event(stream, TestEvent::SuiteFinished {
            total,
            passed,
            failed: if !status.success() && failed == 0 { 1 } else { failed }, // Fallback if parsing failed but exit code bad
        }).await?;

        Ok(())
    }

    async fn send_resp(&self, stream: &mut UnixStream, resp: MitosisResponse) -> Result<()> {
        let bytes = cell_model::rkyv::to_bytes::<_, 256>(&resp)?.into_vec();
        stream.write_all(&(bytes.len() as u32).to_le_bytes()).await?;
        stream.write_all(&bytes).await?;
        Ok(())
    }

    async fn send_event(&self, stream: &mut UnixStream, event: TestEvent) -> Result<()> {
        let bytes = cell_model::rkyv::to_bytes::<_, 1024>(&event)?.into_vec();
        stream.write_all(&(bytes.len() as u32).to_le_bytes()).await?;
        stream.write_all(&bytes).await?;
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_writer(std::io::stderr).init();
    Hypervisor::ignite().await
}