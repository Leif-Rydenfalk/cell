// cell-discovery/src/mdns.rs
// SPDX-License-Identifier: MIT
// mDNS/UDP Multicast Discovery Implementation

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use tokio::net::UdpSocket;
use tokio::time::{interval, Duration, timeout};

const MDNS_MULTICAST_ADDR: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
const MDNS_PORT: u16 = 5353;
const DISCOVERY_INTERVAL: Duration = Duration::from_secs(5);
const SERVICE_TYPE: &str = "_cell._tcp.local";

#[derive(Debug, Clone)]
pub struct MdnsService {
    socket: UdpSocket,
    local_addr: SocketAddr,
}

impl MdnsService {
    pub async fn bind() -> anyhow::Result<Self> {
        // Bind to any available port on all interfaces
        let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0);
        let socket = UdpSocket::bind(bind_addr).await?;
        
        // Join multicast group on all interfaces
        socket.join_multicast_v4(MDNS_MULTICAST_ADDR, Ipv4Addr::UNSPECIFIED)?;
        
        let local_addr = socket.local_addr()?;
        tracing::info!("[mDNS] Bound to {}", local_addr);
        
        Ok(Self { socket, local_addr })
    }
    
    pub async fn announce(&self, cell_name: &str, instance_id: u64, port: u16) {
        let announcement = MdnsAnnouncement {
            cell_name: cell_name.to_string(),
            instance_id,
            port,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        };
        
        let packet = rkyv::to_bytes::<_, 256>(&announcement)
            .expect("serialize announcement")
            .into_vec();
        
        let dest = SocketAddr::new(IpAddr::V4(MDNS_MULTICAST_ADDR), MDNS_PORT);
        
        if let Err(e) = self.socket.send_to(&packet, dest).await {
            tracing::warn!("[mDNS] Failed to announce: {}", e);
        }
    }
    
    pub async fn listen(&self) -> anyhow::Result<MdnsAnnouncement> {
        let mut buf = vec![0u8; 1024];
        
        match timeout(Duration::from_secs(30), self.socket.recv_from(&mut buf)).await {
            Ok(Ok((len, from))) => {
                let packet = &buf[..len];
                let archived = rkyv::check_archived_root::<MdnsAnnouncement>(packet)
                    .map_err(|e| anyhow::anyhow!("Invalid packet from {}: {:?}", from, e))?;
                
                let announcement: MdnsAnnouncement = archived
                    .deserialize(&mut rkyv::de::deserializers::SharedDeserializeMap::new())?;
                
                // Ignore our own announcements
                if from.ip() == self.local_addr.ip() {
                    return Err(anyhow::anyhow!("Self-announcement"));
                }
                
                tracing::debug!("[mDNS] Discovered {} from {}", announcement.cell_name, from);
                Ok(announcement)
            }
            Ok(Err(e)) => Err(e.into()),
            Err(_) => Err(anyhow::anyhow!("Listen timeout")),
        }
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone)]
#[archive(check_bytes)]
pub struct MdnsAnnouncement {
    pub cell_name: String,
    pub instance_id: u64,
    pub port: u16,
    pub timestamp: u64,
}

/// High-level LAN discovery service that combines mDNS with the cache
pub struct LanDiscoveryService {
    mdns: MdnsService,
    cache: crate::lan::LanDiscovery,
    cell_name: String,
    instance_id: u64,
    port: u16,
}

impl LanDiscoveryService {
    pub async fn new(cell_name: &str, port: u16) -> anyhow::Result<Self> {
        let mdns = MdnsService::bind().await?;
        let cache = crate::lan::LanDiscovery::global();
        
        // Generate unique instance ID from hostname + pid + random
        let hostname = std::env::var("HOSTNAME")
            .or_else(|_| std::env::var("COMPUTERNAME"))
            .unwrap_or_else(|_| "unknown".to_string());
        let pid = std::process::id();
        let random: u64 = rand::random();
        let instance_id = blake3::hash(format!("{}:{}:{}", hostname, pid, random).as_bytes())
            .as_bytes()[..8]
            .try_into()
            .map(u64::from_le_bytes)
            .unwrap_or(random);
        
        Ok(Self {
            mdns,
            cache: cache.clone(),
            cell_name: cell_name.to_string(),
            instance_id,
            port,
        })
    }
    
    pub fn start(self) {
        // Spawn announcement task
        let mdns_announce = self.mdns.clone();
        let cell_name = self.cell_name.clone();
        let instance_id = self.instance_id;
        let port = self.port;
        
        tokio::spawn(async move {
            let mut ticker = interval(DISCOVERY_INTERVAL);
            loop {
                ticker.tick().await;
                mdns_announce.announce(&cell_name, instance_id, port).await;
            }
        });
        
        // Spawn listener task
        let cache = self.cache.clone();
        tokio::spawn(async move {
            loop {
                match self.mdns.listen().await {
                    Ok(announcement) => {
                        // Convert to Signal and update cache
                        let sig = crate::lan::Signal {
                            cell_name: announcement.cell_name,
                            instance_id: announcement.instance_id,
                            ip: announcement.ip, // Need to capture from socket
                            port: announcement.port,
                            timestamp: announcement.timestamp,
                            hardware: crate::hardware::HardwareCaps::scan(),
                        };
                        cache.update(sig).await;
                    }
                    Err(e) => {
                        tracing::debug!("[mDNS] Listen error: {}", e);
                    }
                }
            }
        });
    }
    
    pub async fn find(&self, cell_name: &str) -> Option<crate::lan::Signal> {
        self.cache.find_any(cell_name).await
    }
    
    pub async fn find_all(&self, cell_name: &str) -> Vec<crate::lan::Signal> {
        self.cache.find_all(cell_name).await
    }
}