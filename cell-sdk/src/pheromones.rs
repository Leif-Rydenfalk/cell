// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

#![cfg(feature = "axon")]

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::RwLock;

const MULTICAST_ADDR: Ipv4Addr = Ipv4Addr::new(239, 255, 0, 1);
const PORT: u16 = 9099;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Signal {
    pub cell_name: String,
    pub ip: String,
    pub port: u16,
    // Future: Add PubKey for Noise/Snow handshake
}

pub struct PheromoneSystem {
    cache: Arc<RwLock<HashMap<String, Signal>>>,
    socket: Arc<UdpSocket>,
}

impl PheromoneSystem {
    pub async fn ignite() -> Result<Arc<Self>> {
        let socket = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )?;

        socket.set_reuse_address(true)?;

        #[cfg(unix)]
        if let Err(e) = socket.set_reuse_port(true) {
            eprintln!("Warning: SO_REUSEPORT failed: {}", e);
        }

        socket.set_nonblocking(true)?;
        socket.bind(&format!("0.0.0.0:{}", PORT).parse::<SocketAddr>()?.into())?;
        socket.join_multicast_v4(&MULTICAST_ADDR, &Ipv4Addr::UNSPECIFIED)?;

        let udp = UdpSocket::from_std(socket.into())?;
        let sys = Arc::new(Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            socket: Arc::new(udp),
        });

        // Background Listener
        let sys_clone = sys.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 1024];
            loop {
                if let Ok((len, _)) = sys_clone.socket.recv_from(&mut buf).await {
                    if let Ok(sig) = serde_json::from_slice::<Signal>(&buf[..len]) {
                        // Update Cache
                        sys_clone.cache.write().await.insert(sig.cell_name.clone(), sig);
                    }
                }
            }
        });

        Ok(sys)
    }

    /// Automatically resolves local LAN IP
    pub fn local_ip() -> String {
        local_ip_address::local_ip()
            .map(|ip| ip.to_string())
            .unwrap_or_else(|_| "127.0.0.1".to_string())
    }

    /// One-off broadcast
    pub async fn secrete(&self, cell_name: &str, port: u16) -> Result<()> {
        let sig = Signal {
            cell_name: cell_name.into(),
            ip: Self::local_ip(),
            port,
        };
        let bytes = serde_json::to_vec(&sig)?;
        let target = format!("{}:{}", MULTICAST_ADDR, PORT);
        self.socket.send_to(&bytes, target).await?;
        Ok(())
    }

    /// Starts a background task that broadcasts presence every 5 seconds
    pub fn start_secreting(self: &Arc<Self>, cell_name: String, port: u16) {
        let sys = self.clone();
        tokio::spawn(async move {
            loop {
                if let Err(e) = sys.secrete(&cell_name, port).await {
                    eprintln!("[Pheromones] Failed to secrete: {}", e);
                }
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        });
    }

    pub async fn lookup(&self, cell_name: &str) -> Option<Signal> {
        self.cache.read().await.get(cell_name).cloned()
    }
}