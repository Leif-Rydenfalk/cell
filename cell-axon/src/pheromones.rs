// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::Result;
use rkyv::{Archive, Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;
use tokio::sync::RwLock;

const PORT: u16 = 9099;

#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[archive(check_bytes)]
pub struct Signal {
    pub cell_name: String,
    pub ip: String,
    pub port: u16,
    pub timestamp: u64,
}

pub struct PheromoneSystem {
    cache: Arc<RwLock<HashMap<String, Vec<Signal>>>>, 
    socket: Arc<UdpSocket>,
    local_signals: Arc<RwLock<Vec<Signal>>>, 
}

impl PheromoneSystem {
    pub async fn ignite() -> Result<Arc<Self>> {
        let socket = UdpSocket::bind(format!("0.0.0.0:{}", PORT)).await?;
        socket.set_broadcast(true)?;

        let sys = Arc::new(Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            socket: Arc::new(socket),
            local_signals: Arc::new(RwLock::new(Vec::new())),
        });

        let sys_clone = sys.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 1024];
            loop {
                // 1. Receive Data
                let (len, addr) = match sys_clone.socket.recv_from(&mut buf).await {
                    Ok(res) => res,
                    Err(_) => continue,
                };

                // 2. Synchronous Parsing Scope
                // Explicitly annotate type `Signal` here to satisfy the compiler
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

                // 3. Async Logic
                if let Ok(my_ip) = sys_clone.socket.local_addr() {
                    if addr.ip() == my_ip.ip() {
                        continue;
                    }
                }

                if sig.port == 0 {
                    // Query Request
                    let local = sys_clone.local_signals.read().await;
                    for my_sig in local.iter() {
                        if my_sig.cell_name != sig.cell_name {
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
        });

        Ok(sys)
    }

    pub async fn query(&self, target_cell_name: &str) -> Result<()> {
        let sig = Signal {
            cell_name: target_cell_name.into(),
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

    pub async fn secrete(&self, cell_name: &str, port: u16) -> Result<()> {
        let ip = get_best_local_ip();
        self.secrete_specific(cell_name, &ip, port).await
    }

    async fn broadcast_to_all_interfaces(&self, bytes: &[u8]) -> Result<()> {
        let interfaces = if_addrs::get_if_addrs()?;

        for iface in interfaces {
            if iface.is_loopback() {
                continue;
            }

            match iface.addr {
                if_addrs::IfAddr::V4(v4_addr) => {
                    let broadcast = v4_addr
                        .broadcast
                        .unwrap_or_else(|| Ipv4Addr::new(255, 255, 255, 255));

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

    pub async fn lookup(&self, cell_name: &str) -> Option<Signal> {
        self.cache
            .read()
            .await
            .get(cell_name)
            .and_then(|v| v.first())
            .cloned()
    }

    pub async fn lookup_all(&self, cell_name: &str) -> Vec<Signal> {
        self.cache
            .read()
            .await
            .get(cell_name)
            .cloned()
            .unwrap_or_default()
    }
}

fn get_best_local_ip() -> String {
    if let Ok(ip) = std::env::var("CELL_IP") {
        return ip;
    }

    if let Ok(ip) = local_ip_address::local_ip() {
        return ip.to_string();
    }

    if let Ok(interfaces) = if_addrs::get_if_addrs() {
        for iface in interfaces {
            if iface.is_loopback() {
                continue;
            }

            if let if_addrs::IfAddr::V4(v4_addr) = iface.addr {
                let ip = v4_addr.ip;

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