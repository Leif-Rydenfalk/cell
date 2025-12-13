// cells/mycelium/src/main.rs
// SPDX-License-Identifier: MIT
// The Supervisor: Auto-spawns, Heals, and Scales the Mesh.

use anyhow::Result;
use cell_sdk::cell_remote;
use cell_sdk::System;
use cell_model::protocol::{MitosisRequest, MitosisResponse};
use cell_model::rkyv::{self, Deserialize};
use cell_discovery::lan::Signal;
use socket2::{Socket, Domain, Type, Protocol};
use std::collections::{HashSet, HashMap};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;
use tokio::net::UdpSocket;
use tracing::{info, warn, error};

// Remote interface to Hypervisor for spawning
cell_remote!(Hypervisor = "hypervisor");
// Remote interface to Nucleus for health checks
cell_remote!(Nucleus = "nucleus");

const PHEROMONE_PORT: u16 = 9099;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    info!("╔══════════════════════════════════════════════════════════╗");
    info!("║           MYCELIUM SUPERVISOR ONLINE                     ║");
    info!("║   No Ops. No Config. Just Biology.                       ║");
    info!("╚══════════════════════════════════════════════════════════╝");

    // 1. Boot Core Cells (Nucleus, Axon, Hypervisor)
    boot_core_cells().await?;

    // 2. Start Demand Monitor (Auto-spawn on connection attempt)
    let demand_handle = tokio::spawn(monitor_demand());

    // 3. Start Health & Scaling Monitor
    let health_handle = tokio::spawn(monitor_health());

    // Wait forever
    let _ = tokio::join!(demand_handle, health_handle);
    Ok(())
}

async fn boot_core_cells() -> Result<()> {
    info!("[Mycelium] Verifying core infrastructure...");
    
    // We assume Hypervisor acts as the local daemon. 
    // If it's not running, we must ignite it raw.
    // System::spawn inside SDK handles finding/connecting to Hypervisor. 
    // But if Hypervisor is dead, System::spawn fails. 
    // We need a lower-level check: is the socket there?
    
    let home = dirs::home_dir().expect("No HOME");
    let system_dir = home.join(".cell/runtime/system");
    let hypervisor_sock = system_dir.join("mitosis.sock");

    if !hypervisor_sock.exists() || tokio::net::UnixStream::connect(&hypervisor_sock).await.is_err() {
        info!("[Mycelium] Hypervisor missing. Igniting local cluster...");
        // Use the SDK's ignite function which handles raw process spawning
        if let Err(e) = cell_sdk::System::ignite_local_cluster().await {
            error!("[Mycelium] Failed to ignite cluster: {}", e);
            // Fallback: continue, maybe it's just slow
        } else {
            // Give it a moment
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    // Now ensure Nucleus and Axon are running via Hypervisor
    let required = ["nucleus", "axon", "registry", "builder"];
    let mut hypervisor = match Hypervisor::Client::connect().await {
        Ok(c) => c,
        Err(e) => {
            error!("[Mycelium] Critical: Cannot reach Hypervisor: {}", e);
            return Ok(()); // Retry in main loop?
        }
    };

    // We can't query Nucleus yet if it's not up.
    // Blindly try to spawn core cells. Hypervisor handles idempotency (locks).
    for cell in required {
        info!("[Mycelium] Ensuring '{}' is active...", cell);
        let _ = hypervisor.spawn(cell.to_string(), None).await;
    }

    Ok(())
}

async fn monitor_demand() -> Result<()> {
    // Listen to Pheromones on UDP 9099 (Shared Port)
    let socket = bind_reuse_port(PHEROMONE_PORT)?;
    let mut buf = [0u8; 2048];

    // Cache to avoid spamming spawn requests
    let mut recent_spawns: HashMap<String, std::time::Instant> = HashMap::new();

    info!("[Mycelium] Listening for unmet demands (Pheromones)...");

    loop {
        let (len, _addr) = match socket.recv_from(&mut buf).await {
            Ok(res) => res,
            Err(_) => continue,
        };

        // Prune cache
        recent_spawns.retain(|_, t| t.elapsed() < Duration::from_secs(30));

        let sig: Signal = {
            let archived = match rkyv::check_archived_root::<Signal>(&buf[..len]) {
                Ok(a) => a,
                Err(_) => continue, 
            };
            match archived.deserialize(&mut rkyv::Infallible) {
                Ok(s) => s,
                Err(_) => continue,
            }
        };

        // We are interested in Queries (port == 0)
        if sig.port == 0 {
            let target = &sig.cell_name;
            
            // Avoid responding to queries for ourselves or things we just spawned
            if recent_spawns.contains_key(target) {
                continue;
            }

            // Check if it's already running locally?
            // If it was running locally, Axon would have replied.
            // If we hear the query, it means the requester is broadcasting because they didn't find it easily?
            // Actually, requester broadcasts immediately.
            // Mycelium needs to know if it *exists* anywhere.
            // Simple heuristic: If we see a query, check if Nucleus knows about it.
            // If Nucleus says "No instances", then SPAWN IT.

            if let Ok(mut nucleus) = Nucleus::Client::connect().await {
                // If Nucleus is down, we can't do much.
                
                let discovery = nucleus.discover(Nucleus::DiscoveryQuery {
                    cell_name: target.clone(),
                    prefer_local: true,
                }).await;

                let needs_spawn = match discovery {
                    Ok(res) => res.instances.is_empty(),
                    Err(_) => true, // Assume missing if Nucleus fails or returns error
                };

                if needs_spawn {
                    // Check if we have the DNA (binary or source)
                    if has_dna(target) {
                        info!("[Mycelium] Demand detected for '{}'. Auto-spawning...", target);
                        
                        if let Ok(mut hv) = Hypervisor::Client::connect().await {
                            match hv.spawn(target.clone(), None).await {
                                Ok(_) => {
                                    info!("[Mycelium] ✓ Spawned '{}'", target);
                                    recent_spawns.insert(target.clone(), std::time::Instant::now());
                                },
                                Err(e) => {
                                    warn!("[Mycelium] Failed to spawn '{}': {}", target, e);
                                    // Backoff for this target
                                    recent_spawns.insert(target.clone(), std::time::Instant::now()); 
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

async fn monitor_health() -> Result<()> {
    info!("[Mycelium] Health Monitor active...");
    
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;

        let mut nucleus = match Nucleus::Client::connect().await {
            Ok(c) => c,
            Err(_) => continue,
        };

        // Ask Nucleus for status/health
        // Nucleus::status returns managed cells.
        if let Ok(status) = nucleus.status().await {
            // Logic: If a cell is in the "managed" list but has 0 instances in discovery, revive it?
            // Nucleus implementation in this repo is simple; it just lists registered.
            // Let's iterate known cells and ping them.
            
            for cell_name in status.managed_cells {
                // Simple Ping Check via Discovery logic
                // Or verify existence in Nucleus discovery
                let disc = nucleus.discover(Nucleus::DiscoveryQuery { 
                    cell_name: cell_name.clone(), 
                    prefer_local: true 
                }).await;

                if let Ok(res) = disc {
                    if res.instances.is_empty() {
                        // It was registered but now gone? 
                        // Or just checking consistency.
                        // Let's revive it if it's a "known good" service.
                        warn!("[Mycelium] Cell '{}' appears dead. Reviving...", cell_name);
                        if let Ok(mut hv) = Hypervisor::Client::connect().await {
                            let _ = hv.spawn(cell_name, None).await;
                        }
                    }
                }
            }
        }
        
        // Simple Autoscaler Mockup
        // If we had metric access here, we'd check CPU/Queue > Threshold -> Spawn More.
    }
}

fn has_dna(name: &str) -> bool {
    let home = dirs::home_dir().unwrap();
    // Check Registry (Source)
    if home.join(".cell/registry").join(name).exists() { return true; }
    // Check Bin cache
    if home.join(".cell/bin").join(name).exists() { return true; }
    #[cfg(windows)]
    if home.join(".cell/bin").join(format!("{}.exe", name)).exists() { return true; }
    
    false
}

fn bind_reuse_port(port: u16) -> Result<UdpSocket> {
    let domain = Domain::IPV4;
    let socket = Socket::new(domain, Type::DGRAM, Some(Protocol::UDP))?;

    #[cfg(unix)]
    {
        socket.set_reuse_address(true)?;
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "android"))]
        socket.set_reuse_port(true)?;
    }

    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), port);
    socket.bind(&addr.into())?;
    socket.set_nonblocking(true)?;

    let std_sock: std::net::UdpSocket = socket.into();
    UdpSocket::from_std(std_sock).map_err(|e| e.into())
}