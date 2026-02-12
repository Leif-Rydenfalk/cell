// cell-discovery/src/lan.rs (COMPLETE REPLACEMENT)
// SPDX-License-Identifier: MIT
// LAN Discovery via UDP Multicast - Zero Config, Zero Central Registry

use rkyv::{Archive, Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::RwLock;
use tokio::time::{interval, Duration, timeout};
use crate::hardware::HardwareCaps;

const MULTICAST_ADDR: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251); // mDNS standard
const DISCOVERY_PORT: u16 = 5353;
const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(5);
const STALE_THRESHOLD_SECS: u64 = 60;

#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[archive(check_bytes)]
pub struct Signal {
    pub cell_name: String,
    pub instance_id: u64,
    pub ip: String,
    pub port: u16,
    pub timestamp: u64,
    pub hardware: HardwareCaps,
}

pub struct LanDiscovery {
    cache: Arc<RwLock<HashMap<String, HashMap<u64, Signal>>>>,
}

impl LanDiscovery {
    pub fn global() -> &'static Self {
        static INSTANCE: std::sync::OnceLock<LanDiscovery> = std::sync::OnceLock::new();
        INSTANCE.get_or_init(|| {
            let cache = Arc::new(RwLock::new(HashMap::new()));
            Self { cache }
        })
    }

    /// Start announcing this cell's presence and listening for others
    /// Call this once when your cell starts up
    pub fn start_service(&self, cell_name: &str, port: u16) {
        let cell_name = cell_name.to_string();
        let instance_id = Self::generate_instance_id();
        let local_ip = Self::guess_local_ip();
        let cache = self.cache.clone();
        
        tracing::info!(
            "[LAN] Starting discovery for '{}' (instance {}) at {}:{}, multicast {}", 
            cell_name, instance_id, local_ip, port, MULTICAST_ADDR
        );
        
        // Spawn announcement task
        tokio::spawn(async move {
            let socket = match UdpSocket::bind("0.0.0.0:0").await {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("[LAN] Failed to bind announcement socket: {}", e);
                    return;
                }
            };
            
            // Set TTL for multicast
            if let Err(e) = socket.set_ttl(255) {
                tracing::warn!("[LAN] Failed to set TTL: {}", e);
            }
            
            let dest = SocketAddr::new(IpAddr::V4(MULTICAST_ADDR), DISCOVERY_PORT);
            let mut ticker = interval(ANNOUNCE_INTERVAL);
            
            loop {
                ticker.tick().await;
                
                let announcement = Signal {
                    cell_name: cell_name.clone(),
                    instance_id,
                    ip: local_ip.clone(),
                    port,
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                    hardware: HardwareCaps::scan(),
                };
                
                match rkyv::to_bytes::<_, 512>(&announcement) {
                    Ok(bytes) => {
                        if let Err(e) = socket.send_to(bytes.as_slice(), dest).await {
                            tracing::debug!("[LAN] Announce failed: {}", e);
                        } else {
                            tracing::trace!("[LAN] Announced {} at {}:{}", cell_name, local_ip, port);
                        }
                    }
                    Err(e) => tracing::warn!("[LAN] Serialize failed: {}", e),
                }
            }
        });
        
        // Spawn listener task
        let cache = self.cache.clone();
        let my_instance_id = instance_id;
        tokio::spawn(async move {
            // Try to bind to the discovery port - may fail if another cell on this machine has it
            let socket = match UdpSocket::bind(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::UNSPECIFIED), 
                DISCOVERY_PORT
            )).await {
                Ok(s) => {
                    tracing::info!("[LAN] Bound to discovery port {}", DISCOVERY_PORT);
                    s
                }
                Err(e) => {
                    tracing::debug!("[LAN] Could not bind port {} (may be shared): {}", DISCOVERY_PORT, e);
                    // Bind to any port - we'll still receive multicast
                    match UdpSocket::bind("0.0.0.0:0").await {
                        Ok(s) => {
                            tracing::info!("[LAN] Bound to ephemeral port for multicast receive");
                            s
                        }
                        Err(e2) => {
                            tracing::error!("[LAN] Failed to bind any socket: {}", e2);
                            return;
                        }
                    }
                }
            };
            
            // Join multicast group - this is the key for receiving
            if let Err(e) = socket.join_multicast_v4(MULTICAST_ADDR, Ipv4Addr::UNSPECIFIED) {
                tracing::warn!("[LAN] Failed to join multicast group: {}. LAN discovery disabled.", e);
                return;
            }
            
            tracing::info!("[LAN] Listening for multicast announcements on {}", MULTICAST_ADDR);
            
            let mut buf = vec![0u8; 1024];
            
            loop {
                match timeout(Duration::from_secs(60), socket.recv_from(&mut buf)).await {
                    Ok(Ok((len, from))) => {
                        let packet = &buf[..len];
                        
                        match rkyv::check_archived_root::<Signal>(packet) {
                            Ok(archived) => {
                                let sig: Signal = match archived.deserialize(
                                    &mut rkyv::de::deserializers::SharedDeserializeMap::new()
                                ) {
                                    Ok(s) => s,
                                    Err(e) => {
                                        tracing::trace!("[LAN] Deserialization failed: {}", e);
                                        continue;
                                    }
                                };
                                
                                // Ignore self-announcements
                                if sig.instance_id == my_instance_id {
                                    continue;
                                }
                                
                                // Validate timestamp (prevent replay of very old announcements)
                                let now = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_secs();
                                
                                if now.saturating_sub(sig.timestamp) > STALE_THRESHOLD_SECS * 2 {
                                    tracing::trace!("[LAN] Ignoring stale announcement from {}", from);
                                    continue;
                                }
                                
                                tracing::debug!(
                                    "[LAN] Discovered {} (instance {}) at {}:{} (hardware: {} cores, {}MB)", 
                                    sig.cell_name, 
                                    sig.instance_id,
                                    sig.ip,
                                    sig.port,
                                    sig.hardware.cpu_cores,
                                    sig.hardware.total_memory_mb
                                );
                                
                                // Update cache
                                let mut cache_guard = cache.write().await;
                                cache_guard
                                    .entry(sig.cell_name.clone())
                                    .or_insert_with(HashMap::new)
                                    .insert(sig.instance_id, sig);
                            }
                            Err(e) => {
                                tracing::trace!("[LAN] Invalid packet from {}: {:?}", from, e);
                            }
                        }
                    }
                    Ok(Err(e)) => {
                        tracing::debug!("[LAN] Receive error: {}", e);
                    }
                    Err(_) => {
                        // Timeout - normal, just loop and retry
                    }
                }
            }
        });
        
        // Spawn cache pruning task (remove stale entries)
        let cache = self.cache.clone();
        tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(30));
            loop {
                ticker.tick().await;
                
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                
                let mut cache_guard = cache.write().await;
                let mut pruned_count = 0;
                
                // Remove stale entries
                for inner in cache_guard.values_mut() {
                    let before = inner.len();
                    inner.retain(|_, v| {
                        let fresh = now - v.timestamp < STALE_THRESHOLD_SECS;
                        if !fresh { pruned_count += 1; }
                        fresh
                    });
                }
                
                // Remove empty outer keys
                cache_guard.retain(|_, v| !v.is_empty());
                
                if pruned_count > 0 {
                    let total: usize = cache_guard.values().map(|v| v.len()).sum();
                    tracing::debug!("[LAN] Pruned {} stale entries, {} cells, {} instances remain", 
                        pruned_count, cache_guard.len(), total);
                }
            }
        });
    }

    pub async fn all(&self) -> Vec<Signal> {
        let cache = self.cache.read().await;
        cache.values()
            .flat_map(|inner| inner.values())
            .cloned()
            .collect()
    }

    pub async fn find_any(&self, name: &str) -> Option<Signal> {
        let cache = self.cache.read().await;
        cache.get(name)
            .and_then(|inner| inner.values().next())
            .cloned()
    }

    pub async fn find_all(&self, name: &str) -> Vec<Signal> {
        let cache = self.cache.read().await;
        cache.get(name)
            .map(|inner| inner.values().cloned().collect())
            .unwrap_or_default()
    }

    fn generate_instance_id() -> u64 {
        let hostname = std::env::var("HOSTNAME")
            .or_else(|_| std::env::var("COMPUTERNAME"))
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_else(|_| "unknown".to_string());
        let pid = std::process::id();
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        let random: u64 = rand::random();
        
        let hash = blake3::hash(format!("{}:{}:{}:{}", hostname, pid, time, random).as_bytes());
        u64::from_le_bytes(hash.as_bytes()[..8].try_into().unwrap_or([0; 8]))
    }

    fn guess_local_ip() -> String {
        // Try to find a non-loopback IP
        if let Ok(addrs) = if_addrs::get_if_addrs() {
            for iface in addrs {
                if !iface.is_loopback() {
                    if let IpAddr::V4(v4) = iface.ip() {
                        // Skip link-local addresses
                        if !v4.is_link_local() && !v4.is_multicast() {
                            return v4.to_string();
                        }
                    }
                }
            }
        }
        
        // Fallback
        "127.0.0.1".to_string()
    }
}