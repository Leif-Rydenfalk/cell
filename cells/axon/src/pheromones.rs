// cells/axon/src/pheromones.rs
// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::Result;
use cell_model::rkyv::{self, Deserialize};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;
use tokio::sync::RwLock;
use cell_discovery::lan::{Signal, LanDiscovery};
use local_ip_address;
use if_addrs;

const PORT: u16 = 9099;

pub struct PheromoneSystem {
    socket: Arc<UdpSocket>,
    local_signals: Arc<RwLock<Vec<Signal>>>, 
    node_id: u64,
}

impl PheromoneSystem {
    pub async fn ignite(node_id: u64) -> Result<Arc<Self>> {
        let socket = UdpSocket::bind(format!("0.0.0.0:{}", PORT)).await?;
        socket.set_broadcast(true)?;

        let sys = Arc::new(Self {
            socket: Arc::new(socket),
            local_signals: Arc::new(RwLock::new(Vec::new())),
            node_id,
        });

        let sys_clone = sys.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 1024];
            loop {
                let (len, addr) = match sys_clone.socket.recv_from(&mut buf).await {
                    Ok(res) => res,
                    Err(_) => continue,
                };

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

                if let Ok(my_ip) = sys_clone.socket.local_addr() {
                    if addr.ip() == my_ip.ip() {
                        continue;
                    }
                }

                if sig.port == 0 {
                    // Query Request
                    let local = sys_clone.local_signals.read().await;
                    for my_sig in local.iter() {
                        // Respond if we match the queried name
                        if my_sig.cell_name == sig.cell_name {
                            let mut reply = my_sig.clone();
                            reply.timestamp = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs();

                            if let Ok(bytes) = rkyv::to_bytes::<_, 256>(&reply) {
                                let _ = sys_clone.socket.send_to(&bytes, addr).await;
                            }
                        }
                    }
                } else {
                    // Advertisement
                    LanDiscovery::global().update(sig.clone()).await;
                }
            }
        });

        Ok(sys)
    }

    pub async fn query(&self, target_cell_name: &str) -> Result<()> {
        let sig = Signal {
            cell_name: target_cell_name.into(),
            instance_id: self.node_id,
            ip: get_best_local_ip(),
            port: 0,
            timestamp: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        };
        let bytes = rkyv::to_bytes::<_, 256>(&sig)?;
        self.broadcast_to_all_interfaces(&bytes).await
    }

    pub async fn secrete_specific(&self, cell_name: &str, ip: &str, port: u16) -> Result<()> {
        let sig = Signal {
            cell_name: cell_name.into(),
            instance_id: self.node_id,
            ip: ip.to_string(),
            port,
            timestamp: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        };

        {
            let mut local = self.local_signals.write().await;
            local.retain(|s| !(s.cell_name == cell_name && s.ip == ip && s.port == port));
            local.push(sig.clone());
        }

        let bytes = rkyv::to_bytes::<_, 256>(&sig)?;
        self.broadcast_to_all_interfaces(&bytes).await
    }

    #[allow(dead_code)]
    pub async fn secrete(&self, cell_name: &str, port: u16) -> Result<()> {
        let ip = get_best_local_ip();
        self.secrete_specific(cell_name, &ip, port).await
    }

    async fn broadcast_to_all_interfaces(&self, bytes: &[u8]) -> Result<()> {
        let interfaces = if_addrs::get_if_addrs()?;
        for iface in interfaces {
            if iface.is_loopback() { continue; }
            match iface.addr {
                if_addrs::IfAddr::V4(v4_addr) => {
                    let broadcast = v4_addr.broadcast.unwrap_or_else(|| Ipv4Addr::new(255, 255, 255, 255));
                    let target = SocketAddr::new(IpAddr::V4(broadcast), PORT);
                    let _ = self.socket.send_to(bytes, target).await;
                }
                if_addrs::IfAddr::V6(_v6_addr) => {
                    let multicast = std::net::Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 1);
                    let target = SocketAddr::new(IpAddr::V6(multicast), PORT);
                    let _ = self.socket.send_to(bytes, target).await;
                }
            }
        }
        Ok(())
    }

    pub fn start_secreting(self: &Arc<Self>, _cell_name: String, _port: u16) {
        let sys = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let signals = sys.local_signals.read().await.clone();
                for sig in signals {
                    if let Ok(bytes) = rkyv::to_bytes::<_, 256>(&sig) {
                        let _ = sys.broadcast_to_all_interfaces(&bytes).await;
                    }
                }
            }
        });
    }

    pub async fn lookup_all(&self, cell_name: &str) -> Vec<Signal> {
        LanDiscovery::global().find_all(cell_name).await
    }
}

fn get_best_local_ip() -> String {
    if let Ok(ip) = std::env::var("CELL_IP") {
        return ip;
    }
    if let Ok(ip) = local_ip_address::local_ip() {
        return ip.to_string();
    }
    "127.0.0.1".to_string()
}