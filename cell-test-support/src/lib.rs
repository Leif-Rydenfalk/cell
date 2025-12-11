// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use cell_sdk::*;
use cell_process::MyceliumRoot;
use cell_model::protocol::{MitosisRequest, MitosisResponse};
use tokio::sync::OnceCell;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use std::sync::Arc;
use tracing::info;

static ROOT: OnceCell<Arc<MyceliumRoot>> = OnceCell::const_new();

/// Initializes the Mycelium Root singleton for testing.
pub async fn root() -> &'static Arc<MyceliumRoot> {
    ROOT.get_or_init(|| async {
        // Force test sockets
        if std::env::var("CELL_SOCKET_DIR").is_err() {
            let mut target_dir = std::env::current_dir().unwrap();
            target_dir.push("target");
            target_dir.push("test-sockets");
            std::fs::create_dir_all(&target_dir).unwrap();
            std::env::set_var("CELL_SOCKET_DIR", target_dir.to_str().unwrap());
        }
        MyceliumRoot::ignite().await.expect("Failed to start Mycelium Root")
    }).await
}

/// Helper to request spawn from Root. Returns a Synapse connected to the cell.
pub async fn spawn(cell_name: &str) -> Synapse {
    let _ = root().await;
    let socket_dir = cell_transport::resolve_socket_dir();
    let umbilical = socket_dir.join("mitosis.sock");
    
    let mut stream = None;
    for _ in 0..50 {
        if let Ok(s) = UnixStream::connect(&umbilical).await {
            stream = Some(s);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let mut stream = stream.expect("Failed to connect to Umbilical");

    let req = MitosisRequest::Spawn { cell_name: cell_name.to_string() };
    let req_bytes = cell_model::rkyv::to_bytes::<_, 256>(&req).unwrap().into_vec();
    stream.write_all(&(req_bytes.len() as u32).to_le_bytes()).await.unwrap();
    stream.write_all(&req_bytes).await.unwrap();

    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await.unwrap();
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut resp_buf = vec![0u8; len];
    stream.read_exact(&mut resp_buf).await.unwrap();

    let resp = cell_model::rkyv::check_archived_root::<MitosisResponse>(&resp_buf).unwrap()
        .deserialize(&mut cell_model::rkyv::Infallible).unwrap();

    match resp {
        MitosisResponse::Ok { .. } => {
            // Poll for socket availability
            for _ in 0..100 {
                if let Ok(syn) = Synapse::grow(cell_name).await {
                    return syn;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            panic!("Cell {} spawned but never became reachable via Synapse", cell_name);
        }
        MitosisResponse::Denied { reason } => panic!("Spawn denied: {}", reason),
    }
}

pub async fn spawn_worker_with_fake_cpu(cpu: f64) {
    // In a real implementation this would mock metrics. 
    // Here we just spawn a worker to ensure capacity increases.
    let _ = spawn("worker").await;
}

pub async fn kill_any(count: usize) {
    // Stub for chaos testing
}

pub async fn nucleus() -> NucleusClient {
    NucleusClient::connect().await.unwrap()
}

pub async fn wait_leader(_nodes: &[Synapse]) -> Synapse {
    // Stub: Return first node, assume it's leader for now
    // A real implementation queries status
    spawn("consensus").await // Just return a fresh connection for the example API
}

pub async fn corrupt_vault_file(_name: &str, _ver: u64) {
    // Stub
}

pub async fn corrupt_observer_log(_id: String) {
    // Stub
}

pub async fn corrupt_audit_entry(_id: u64) {
    // Stub
}