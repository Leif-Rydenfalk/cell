// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

#![cfg(feature = "axon")]

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;
use tokio::sync::RwLock;

const MULTICAST_ADDR_V4: Ipv4Addr = Ipv4Addr::new(239, 255, 0, 1);
const MULTICAST_ADDR_V6: Ipv6Addr = Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 1);
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
    sockets: Vec<Arc<UdpSocket>>,
    // Stores the local configuration if this node is "secreting"
    local_signal: Arc<RwLock<Option<Signal>>>,
}

impl PheromoneSystem {
    pub async fn ignite() -> Result<Arc<Self>> {
        let mut sockets = Vec::new();
        let iface_str = std::env::var("CELL_IFACE").unwrap_or_else(|_| "0.0.0.0".to_string());

        // --- 1. Setup IPv4 Socket ---
        match setup_socket_v4(&iface_str) {
            Ok(s) => {
                // println!("[Pheromones] 🔥 IPv4 Active on interface: {}", iface_str);
                sockets.push(Arc::new(s));
            }
            Err(e) => eprintln!("[Pheromones] Warning: Failed to bind IPv4: {}", e),
        }

        // --- 2. Setup IPv6 Socket ---
        if iface_str == "0.0.0.0" {
            match setup_socket_v6() {
                Ok(s) => {
                    // println!("[Pheromones] 🔥 IPv6 Active");
                    sockets.push(Arc::new(s));
                }
                Err(_) => {}
            }
        }

        if sockets.is_empty() {
            anyhow::bail!("Failed to bind any network sockets for discovery");
        }

        let sys = Arc::new(Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            sockets,
            local_signal: Arc::new(RwLock::new(None)),
        });

        // --- 3. Spawn Listeners for all sockets ---
        for socket in &sys.sockets {
            let sock_clone = socket.clone();
            let sys_clone = sys.clone();

            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                loop {
                    if let Ok((len, addr)) = sock_clone.recv_from(&mut buf).await {
                        if let Ok(sig) = serde_json::from_slice::<Signal>(&buf[..len]) {
                            // ACTIVE DISCOVERY: Check if it's a query (port == 0)
                            if sig.port == 0 {
                                // If we are a server (have local_signal), reply immediately
                                let local = sys_clone.local_signal.read().await;
                                if let Some(my_sig) = local.as_ref() {
                                    // Don't reply to self if we happen to receive our own query
                                    if my_sig.cell_name != sig.cell_name {
                                        // Update timestamp for freshness
                                        let mut reply = my_sig.clone();
                                        if let Ok(ts) = SystemTime::now().duration_since(UNIX_EPOCH)
                                        {
                                            reply.timestamp = ts.as_secs();
                                        }

                                        if let Ok(bytes) = serde_json::to_vec(&reply) {
                                            // Send unicast reply back to querier
                                            let _ = sock_clone.send_to(&bytes, addr).await;
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
        }

        Ok(sys)
    }

    /// Sends a query packet to force immediate discovery
    pub async fn query(&self, target_cell_name: &str) -> Result<()> {
        let sig = Signal {
            cell_name: target_cell_name.into(),
            ip: Self::local_ip(),
            port: 0, // 0 indicates QUERY
            timestamp: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        };
        let bytes = serde_json::to_vec(&sig)?;
        self.send_broadcast(&bytes).await
    }

    /// Resolves local LAN IP, or uses override from env var
    pub fn local_ip() -> String {
        if let Ok(ip) = std::env::var("CELL_IP") {
            return ip;
        }
        local_ip_address::local_ip()
            .map(|ip| ip.to_string())
            .unwrap_or_else(|_| "127.0.0.1".to_string())
    }

    pub async fn secrete(&self, cell_name: &str, port: u16) -> Result<()> {
        let sig = Signal {
            cell_name: cell_name.into(),
            ip: Self::local_ip(),
            port,
            timestamp: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        };

        // Store active config for query replies
        {
            let mut local = self.local_signal.write().await;
            *local = Some(sig.clone());
        }

        let bytes = serde_json::to_vec(&sig)?;
        self.send_broadcast(&bytes).await
    }

    async fn send_broadcast(&self, bytes: &[u8]) -> Result<()> {
        for socket in &self.sockets {
            let local_addr = socket.local_addr()?;
            if local_addr.is_ipv4() {
                // Multicast
                let target_mc = format!("{}:{}", MULTICAST_ADDR_V4, PORT);
                let _ = socket.send_to(bytes, &target_mc).await;
                // Broadcast Fallback
                let target_bc = format!("255.255.255.255:{}", PORT);
                let _ = socket.send_to(bytes, &target_bc).await;
            } else {
                let target_v6 = format!("[{}]:{}", MULTICAST_ADDR_V6, PORT);
                let _ = socket.send_to(bytes, &target_v6).await;
            }
        }
        Ok(())
    }

    pub fn start_secreting(self: &Arc<Self>, cell_name: String, port: u16) {
        let sys = self.clone();
        tokio::spawn(async move {
            // Secrete immediately on start
            let _ = sys.secrete(&cell_name, port).await;

            loop {
                tokio::time::sleep(Duration::from_secs(5)).await;
                if let Err(e) = sys.secrete(&cell_name, port).await {
                    eprintln!("[Pheromones] Failed to secrete: {}", e);
                }
            }
        });
    }

    pub async fn lookup(&self, cell_name: &str) -> Option<Signal> {
        self.cache.read().await.get(cell_name).cloned()
    }
}

fn setup_socket_v4(iface_str: &str) -> Result<UdpSocket> {
    let socket = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )?;

    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    let _ = socket.set_reuse_port(true);

    socket.set_broadcast(true)?;
    socket.set_multicast_loop_v4(true)?;
    socket.set_multicast_ttl_v4(2)?;
    socket.set_nonblocking(true)?;

    let addr: SocketAddr = format!("0.0.0.0:{}", PORT).parse()?;
    socket.bind(&addr.into())?;

    let iface_ip: Ipv4Addr = iface_str.parse()?;

    if iface_ip.is_unspecified() {
        socket.join_multicast_v4(&MULTICAST_ADDR_V4, &Ipv4Addr::UNSPECIFIED)?;
        let _ = socket.join_multicast_v4(&MULTICAST_ADDR_V4, &Ipv4Addr::new(127, 0, 0, 1));
    } else {
        socket.join_multicast_v4(&MULTICAST_ADDR_V4, &iface_ip)?;
    }

    Ok(UdpSocket::from_std(socket.into())?)
}

fn setup_socket_v6() -> Result<UdpSocket> {
    let socket = socket2::Socket::new(
        socket2::Domain::IPV6,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )?;

    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    let _ = socket.set_reuse_port(true);

    socket.set_only_v6(true)?;
    socket.set_multicast_loop_v6(true)?;
    socket.set_nonblocking(true)?;

    let addr: SocketAddr = format!("[::]:{}", PORT).parse()?;
    socket.bind(&addr.into())?;

    socket.join_multicast_v6(&MULTICAST_ADDR_V6, 0)?;

    Ok(UdpSocket::from_std(socket.into())?)
}
