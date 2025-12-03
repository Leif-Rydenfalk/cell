// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

#![cfg(feature = "axon")]

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;
use tokio::sync::RwLock;

const PORT: u16 = 9099;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Signal {
    pub cell_name: String,
    pub ip: String,
    pub port: u16,
    pub timestamp: u64,
}

pub struct PheromoneSystem {
    cache: Arc<RwLock<HashMap<String, Signal>>>,
    socket: Arc<UdpSocket>,
    local_signal: Arc<RwLock<Option<Signal>>>,
}

impl PheromoneSystem {
    pub async fn ignite() -> Result<Arc<Self>> {
        // Bind to 0.0.0.0 to receive from ANY interface
        let socket = UdpSocket::bind(format!("0.0.0.0:{}", PORT)).await?;
        socket.set_broadcast(true)?;

        // Attempt to reuse port so multiple cells can run on one machine
        // Note: In a real prod setup we'd build the socket with socket2 fully
        // to ensure SO_REUSEPORT is set before bind. This default impl relies on OS behavior.

        let sys = Arc::new(Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            socket: Arc::new(socket),
            local_signal: Arc::new(RwLock::new(None)),
        });

        // --- Receiver Loop ---
        let sys_clone = sys.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 1024];
            loop {
                if let Ok((len, addr)) = sys_clone.socket.recv_from(&mut buf).await {
                    if let Ok(sig) = serde_json::from_slice::<Signal>(&buf[..len]) {
                        // Ignore our own echoes
                        if let Ok(my_ip) = sys_clone.socket.local_addr() {
                            if addr.ip() == my_ip.ip() && addr.port() == my_ip.port() {
                                continue;
                            }
                        }

                        // ACTIVE DISCOVERY: Check if it's a query (port == 0)
                        if sig.port == 0 {
                            let local = sys_clone.local_signal.read().await;
                            if let Some(my_sig) = local.as_ref() {
                                if my_sig.cell_name != sig.cell_name {
                                    let mut reply = my_sig.clone();
                                    reply.timestamp = SystemTime::now()
                                        .duration_since(UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_secs();

                                    if let Ok(bytes) = serde_json::to_vec(&reply) {
                                        let _ = sys_clone.socket.send_to(&bytes, addr).await;
                                    }
                                }
                            }
                        } else {
                            // Standard Advertisement
                            crate::discovery::LanDiscovery::global()
                                .update(sig.clone())
                                .await;

                            sys_clone
                                .cache
                                .write()
                                .await
                                .insert(sig.cell_name.clone(), sig);
                        }
                    }
                }
            }
        });

        Ok(sys)
    }

    pub async fn query(&self, target_cell_name: &str) -> Result<()> {
        let sig = Signal {
            cell_name: target_cell_name.into(),
            ip: Self::resolve_best_ip(),
            port: 0,
            timestamp: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        };
        let bytes = serde_json::to_vec(&sig)?;
        self.shotgun_broadcast(&bytes).await
    }

    pub async fn secrete(&self, cell_name: &str, port: u16) -> Result<()> {
        let sig = Signal {
            cell_name: cell_name.into(),
            ip: Self::resolve_best_ip(),
            port,
            timestamp: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        };

        {
            let mut local = self.local_signal.write().await;
            *local = Some(sig.clone());
        }

        let bytes = serde_json::to_vec(&sig)?;
        self.shotgun_broadcast(&bytes).await
    }

    /// The "Shotgun": Sends packets to the broadcast address of EVERY interface.
    /// This bypasses Docker bridge isolation and routing issues.
    async fn shotgun_broadcast(&self, bytes: &[u8]) -> Result<()> {
        let interfaces = if_addrs::get_if_addrs()?;

        for iface in interfaces {
            // Filter out loopback
            if iface.is_loopback() {
                continue;
            }

            // Match if_addrs::IfAddr enum
            if let if_addrs::IfAddr::V4(v4_addr) = iface.addr {
                // Use the specific broadcast address for this interface if available
                let broadcast = v4_addr
                    .broadcast
                    .unwrap_or_else(|| Ipv4Addr::new(255, 255, 255, 255));

                let target = format!("{}:{}", broadcast, PORT);
                let _ = self.socket.send_to(bytes, &target).await;
            }
        }
        Ok(())
    }

    pub fn start_secreting(self: &Arc<Self>, cell_name: String, port: u16) {
        let sys = self.clone();
        tokio::spawn(async move {
            let _ = sys.secrete(&cell_name, port).await;
            loop {
                tokio::time::sleep(Duration::from_secs(3)).await; // Faster pulse
                if let Err(e) = sys.secrete(&cell_name, port).await {
                    eprintln!("[Pheromones] Failed to secrete: {}", e);
                }
            }
        });
    }

    pub async fn lookup(&self, cell_name: &str) -> Option<Signal> {
        self.cache.read().await.get(cell_name).cloned()
    }

    /// Helper to find the "real" LAN IP (not docker0 or localhost) to advertise
    fn resolve_best_ip() -> String {
        if let Ok(ip) = std::env::var("CELL_IP") {
            return ip;
        }

        // Heuristic: Find first non-loopback IPv4
        if let Ok(interfaces) = if_addrs::get_if_addrs() {
            for iface in interfaces {
                if iface.is_loopback() {
                    continue;
                }

                if let if_addrs::IfAddr::V4(v4_addr) = iface.addr {
                    let ip = v4_addr.ip;
                    // Primitive filter to avoid Docker default gateway (usually 172.17.x.1)
                    if ip.octets()[0] == 172 && ip.octets()[1] == 17 && ip.octets()[3] == 1 {
                        continue;
                    }
                    return ip.to_string();
                }
            }
        }

        "127.0.0.1".to_string()
    }
}
