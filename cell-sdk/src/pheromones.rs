// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

#![cfg(feature = "axon")]

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
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
    cache: Arc<RwLock<HashMap<String, Vec<Signal>>>>, // Changed to Vec to store multiple addresses
    socket: Arc<UdpSocket>,
    local_signals: Arc<RwLock<Vec<Signal>>>, // Changed to Vec for multi-address support
}

impl PheromoneSystem {
    pub async fn ignite() -> Result<Arc<Self>> {
        // Bind to 0.0.0.0 to receive from ANY interface
        let socket = UdpSocket::bind(format!("0.0.0.0:{}", PORT)).await?;
        socket.set_broadcast(true)?;

        let sys = Arc::new(Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            socket: Arc::new(socket),
            local_signals: Arc::new(RwLock::new(Vec::new())),
        });

        // Receiver Loop
        let sys_clone = sys.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 1024];
            loop {
                if let Ok((len, addr)) = sys_clone.socket.recv_from(&mut buf).await {
                    if let Ok(sig) = serde_json::from_slice::<Signal>(&buf[..len]) {
                        // Ignore our own echoes
                        if let Ok(my_ip) = sys_clone.socket.local_addr() {
                            if addr.ip() == my_ip.ip() {
                                continue;
                            }
                        }

                        // ACTIVE DISCOVERY: Check if it's a query (port == 0)
                        if sig.port == 0 {
                            let local = sys_clone.local_signals.read().await;
                            for my_sig in local.iter() {
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
                            // Standard Advertisement - store in cache
                            crate::discovery::LanDiscovery::global()
                                .update(sig.clone())
                                .await;

                            sys_clone
                                .cache
                                .write()
                                .await
                                .entry(sig.cell_name.clone())
                                .or_insert_with(Vec::new)
                                .push(sig);
                        }
                    }
                }
            }
        });

        Ok(sys)
    }

    /// Query for a specific cell (active discovery)
    pub async fn query(&self, target_cell_name: &str) -> Result<()> {
        let sig = Signal {
            cell_name: target_cell_name.into(),
            ip: get_best_local_ip(),
            port: 0,
            timestamp: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        };
        let bytes = serde_json::to_vec(&sig)?;
        self.broadcast_to_all_interfaces(&bytes).await
    }

    /// Secrete pheromone for a specific IP:port combination
    pub async fn secrete_specific(&self, cell_name: &str, ip: &str, port: u16) -> Result<()> {
        let sig = Signal {
            cell_name: cell_name.into(),
            ip: ip.to_string(),
            port,
            timestamp: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        };

        // Add to local signals
        {
            let mut local = self.local_signals.write().await;
            // Remove old entry for this specific IP:port combo
            local.retain(|s| !(s.cell_name == cell_name && s.ip == ip && s.port == port));
            local.push(sig.clone());
        }

        let bytes = serde_json::to_vec(&sig)?;
        self.broadcast_to_all_interfaces(&bytes).await
    }

    /// Legacy method for compatibility
    pub async fn secrete(&self, cell_name: &str, port: u16) -> Result<()> {
        let ip = get_best_local_ip();
        self.secrete_specific(cell_name, &ip, port).await
    }

    /// Broadcast to EVERY interface's broadcast address
    async fn broadcast_to_all_interfaces(&self, bytes: &[u8]) -> Result<()> {
        let interfaces = if_addrs::get_if_addrs()?;

        for iface in interfaces {
            if iface.is_loopback() {
                continue;
            }

            match iface.addr {
                if_addrs::IfAddr::V4(v4_addr) => {
                    // Use specific broadcast address for this interface
                    let broadcast = v4_addr
                        .broadcast
                        .unwrap_or_else(|| Ipv4Addr::new(255, 255, 255, 255));

                    let target = SocketAddr::new(IpAddr::V4(broadcast), PORT);
                    let _ = self.socket.send_to(bytes, target).await;
                }
                if_addrs::IfAddr::V6(v6_addr) => {
                    // IPv6 multicast
                    let multicast = std::net::Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 1);
                    let target = SocketAddr::new(IpAddr::V6(multicast), PORT);
                    let _ = self.socket.send_to(bytes, target).await;
                }
            }
        }
        Ok(())
    }

    /// Start continuous secreting (background task)
    pub fn start_secreting(self: &Arc<Self>, cell_name: String, _port: u16) {
        let sys = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;

                // Re-advertise all our local signals
                let signals = sys.local_signals.read().await.clone();
                for sig in signals {
                    if let Ok(bytes) = serde_json::to_vec(&sig) {
                        let _ = sys.broadcast_to_all_interfaces(&bytes).await;
                    }
                }
            }
        });
    }

    /// Lookup a single signal (returns first found)
    pub async fn lookup(&self, cell_name: &str) -> Option<Signal> {
        self.cache
            .read()
            .await
            .get(cell_name)
            .and_then(|v| v.first())
            .cloned()
    }

    /// Lookup ALL signals for a cell (for multi-address discovery)
    pub async fn lookup_all(&self, cell_name: &str) -> Vec<Signal> {
        self.cache
            .read()
            .await
            .get(cell_name)
            .cloned()
            .unwrap_or_default()
    }
}

/// Get the best local IP for advertising
fn get_best_local_ip() -> String {
    // 1. Try environment variable override
    if let Ok(ip) = std::env::var("CELL_IP") {
        return ip;
    }

    // 2. Try local_ip_address crate (most reliable)
    if let Ok(ip) = local_ip_address::local_ip() {
        return ip.to_string();
    }

    // 3. Heuristic: Find first non-loopback IPv4
    if let Ok(interfaces) = if_addrs::get_if_addrs() {
        for iface in interfaces {
            if iface.is_loopback() {
                continue;
            }

            if let if_addrs::IfAddr::V4(v4_addr) = iface.addr {
                let ip = v4_addr.ip;

                // Filter out Docker, link-local, etc.
                if ip.octets()[0] == 172 && ip.octets()[1] == 17 {
                    continue;
                }
                if ip.octets()[0] == 169 && ip.octets()[1] == 254 {
                    continue;
                }

                return ip.to_string();
            }
        }
    }

    "127.0.0.1".to_string()
}
