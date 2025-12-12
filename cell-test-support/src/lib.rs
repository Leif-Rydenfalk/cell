// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use cell_sdk::*;
use cell_process::MyceliumRoot;
use cell_model::protocol::{MitosisRequest, MitosisResponse};
use cell_model::config::{CellInitConfig, PeerConfig};
use tokio::sync::OnceCell;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use std::sync::Arc;
use std::collections::HashMap;
use cell_sdk::rkyv::Deserialize;

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
        
        let root = MyceliumRoot::ignite().await.expect("Failed to start Mycelium Root");
        Arc::new(root)
    }).await
}

/// Helper to request spawn from Root. Returns a Synapse connected to the cell.
pub async fn spawn(cell_name: &str) -> Synapse {
    spawn_with_config(cell_name, HashMap::new()).await
}

/// Spawn with specific configuration (environment variables)
pub async fn spawn_with_config(cell_name: &str, config: HashMap<String, String>) -> Synapse {
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

    // ORCHESTRATOR LOGIC (Test Harness implementation)
    // Map the string configuration to the strict CellInitConfig
    // NOTE: This logic was previously hardcoded in the SDK. It has been moved here
    // because specific topology knowledge belongs in the deployment/test layer, not the library.
    let init_config = if let Some(identity_str) = config.get("CELL_IDENTITY") {
        match identity_str.as_str() {
            "Alpha" => CellInitConfig {
                node_id: 1,
                cell_name: cell_name.to_string(),
                peers: vec![
                    PeerConfig { node_id: 2, address: "Beta".to_string() },
                    PeerConfig { node_id: 3, address: "Gamma".to_string() },
                ],
                socket_path: format!("/tmp/cell/{}.sock", cell_name),
            },
            "Beta" => CellInitConfig {
                node_id: 2,
                cell_name: cell_name.to_string(),
                peers: vec![
                    PeerConfig { node_id: 1, address: "Alpha".to_string() },
                    PeerConfig { node_id: 3, address: "Gamma".to_string() },
                ],
                socket_path: format!("/tmp/cell/{}.sock", cell_name),
            },
            "Gamma" => CellInitConfig {
                node_id: 3,
                cell_name: cell_name.to_string(),
                peers: vec![
                    PeerConfig { node_id: 1, address: "Alpha".to_string() },
                    PeerConfig { node_id: 2, address: "Beta".to_string() },
                ],
                socket_path: format!("/tmp/cell/{}.sock", cell_name),
            },
            _ => {
                // Fallback for custom named identities
                CellInitConfig {
                    node_id: rand::random(),
                    cell_name: cell_name.to_string(),
                    peers: vec![],
                    socket_path: format!("/tmp/cell/{}.sock", cell_name),
                }
            }
        }
    } else {
        // No Identity provided, generic dynamic node
        CellInitConfig {
            node_id: rand::random(),
            cell_name: cell_name.to_string(),
            peers: vec![],
            socket_path: format!("/tmp/cell/{}.sock", cell_name),
        }
    };

    let req = MitosisRequest::Spawn { 
        cell_name: cell_name.to_string(),
        config: Some(init_config),
    };
    
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

pub async fn spawn_worker_with_fake_cpu(_cpu: f64) {
    let _ = spawn("worker").await;
}

pub async fn kill_any(_count: usize) {
    // Stub
}

pub async fn nucleus() -> NucleusClient {
    NucleusClient::connect().await.unwrap()
}

pub async fn wait_leader(_nodes: &[Synapse]) -> Synapse {
    spawn("consensus").await 
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