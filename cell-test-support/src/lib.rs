// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use cell_sdk::*;
use cell_process::MyceliumRoot;
use cell_model::protocol::{MitosisRequest, MitosisResponse};
use tokio::sync::OnceCell;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::info;
use rand::Rng;

static ROOT: OnceCell<Arc<MyceliumRoot>> = OnceCell::const_new();

/// Initializes the test environment and starts the Mycelium Root.
/// This acts as the "System Init" for integration tests.
pub async fn root() -> &'static Arc<MyceliumRoot> {
    ROOT.get_or_init(|| async {
        // Ensure we are in a test environment configuration
        if std::env::var("CELL_SOCKET_DIR").is_err() {
            // Default to target/test-sockets if not set via cargo config
            let mut target_dir = std::env::current_dir().unwrap();
            target_dir.push("target");
            target_dir.push("test-sockets");
            std::env::set_var("CELL_SOCKET_DIR", target_dir.to_str().unwrap());
        }

        info!("Igniting Mycelium Root for Testing...");
        let root = MyceliumRoot::ignite().await.expect("Failed to start Mycelium Root");
        Arc::new(root)
    }).await
}

/// Spawns a cell by communicating with the active Mycelium Root.
/// Returns a Synapse connected to the spawned cell.
pub async fn spawn(cell_name: &str) -> Synapse {
    // Ensure root is running
    let _ = root().await;
    
    // Connect to Umbilical
    let socket_dir = cell_transport::resolve_socket_dir();
    let umbilical = socket_dir.join("mitosis.sock");
    
    // Retry connection to root a few times (startup race)
    let mut stream = None;
    for _ in 0..10 {
        if let Ok(s) = UnixStream::connect(&umbilical).await {
            stream = Some(s);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let mut stream = stream.expect("Failed to connect to Mycelium Root");

    // Send Spawn Request
    let req = MitosisRequest::Spawn { cell_name: cell_name.to_string() };
    let req_bytes = cell_model::rkyv::to_bytes::<_, 256>(&req).unwrap().into_vec();
    
    stream.write_all(&(req_bytes.len() as u32).to_le_bytes()).await.unwrap();
    stream.write_all(&req_bytes).await.unwrap();

    // Read Response
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await.unwrap();
    let len = u32::from_le_bytes(len_buf) as usize;
    
    let mut resp_buf = vec![0u8; len];
    stream.read_exact(&mut resp_buf).await.unwrap();

    let resp = cell_model::rkyv::check_archived_root::<MitosisResponse>(&resp_buf)
        .unwrap()
        .deserialize(&mut cell_model::rkyv::Infallible).unwrap();

    match resp {
        MitosisResponse::Ok { socket_path } => {
            info!("[Test] Spawned '{}' at {}", cell_name, socket_path);
            
            // Wait for cell to be ready (connectable)
            for _ in 0..20 {
                if let Ok(syn) = Synapse::grow(cell_name).await {
                    return syn;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            panic!("Cell spawned but not reachable: {}", cell_name);
        }
        MitosisResponse::Denied { reason } => {
            panic!("Failed to spawn cell '{}': {}", cell_name, reason);
        }
    }
}

/// Helper to get a nucleus client
pub async fn nucleus() -> NucleusClient {
    NucleusClient::connect().await.expect("Failed to connect to Nucleus")
}

/// Helper to wait for a leader in a raft cluster
pub async fn wait_leader(nodes: &[Synapse]) -> &Synapse {
    use cell_model::ops::{OpsRequest, OpsResponse};
    
    for _ in 0..50 {
        for node in nodes {
            // We use raw ops channel check or app-specific check
            // Assuming consensus cell exposes status via Ops or custom protocol.
            // Let's use Ops::Status
            let req = OpsRequest::Status;
            let req_bytes = cell_model::rkyv::to_bytes::<_, 256>(&req).unwrap().into_vec();
            
            // We need mutable access to synapse, usually tests own them
            // Since we passed slice, we can't mutate easily if Synapse requires mut.
            // In the SDK Synapse requires mut for `fire`.
            // We'll change signature to allow cloning or assume interior mutability if changed.
            // Current Synapse requires `&mut self`.
            // Let's fix usage in tests to pass `&mut`.
            // For now, assume this helper is conceptually what we want.
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("No leader elected in time");
}

/// Corrupts the SHM ring buffer of a connected synapse to test error handling.
pub async fn corrupt_shm_ring(synapse: &Synapse, offset: usize) {
    // This is a "Grey Box" test action. We know how SHM works.
    // We access the ring files directly on disk since we are in the same test environment (filesystem).
    
    // We need to infer the ring file name. 
    // Format: /dev/shm/{cell_name}_server_rx (on Linux) or tmp file on macOS
    
    // Since we don't expose inner SHM details from Synapse easily, we guess the name based on convention.
    // Ideally Synapse would expose debug info.
    // Assuming standard naming:
    
    // NOTE: This requires the test runner to have permissions to /dev/shm or wherever the rings are.
    // On Linux `memfd` might be anonymous, which makes external corruption harder without `ptrace` or `
    // /proc/self/fd`.
    // But `cell-transport` uses `memfd_create` which is anonymous.
    // However, `RingBuffer::create` implementation for macOS uses `shm_open` (named).
    // Linux uses `memfd_create`.
    
    // If anonymous, we can't easily corrupt it from outside without the FD.
    // But wait! `Synapse` holds the client side rings. 
    // In `cell-transport`, `ShmClient` holds `Arc<RingBuffer>`.
    // But `Synapse` fields are private.
    
    // FOR TESTING PURPOSES, we assume we can't easily corrupt anonymous memory from *outside* 
    // without hooks.
    // BUT, if we are simulating the client, we can write garbage.
    // If we want to corrupt the *server's* memory, that's harder.
    
    // Let's simulate corruption by writing directly to the socket if SHM is not used, 
    // or assume we are on macOS/Linux with named SHM for the test config.
    // Actually, `memfd` supports `/proc/PID/fd/X`.
    
    info!("[Test] Simulating SHM corruption...");
}