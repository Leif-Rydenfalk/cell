// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
pub struct HealthStatus {
    pub is_healthy: bool,
    pub last_check: Instant,
    pub consecutive_failures: u32,
    pub latency: Duration,
}

pub struct HealthChecker {
    statuses: Arc<RwLock<HashMap<String, HealthStatus>>>,
    check_interval: Duration,
    failure_threshold: u32,
}

impl HealthChecker {
    pub fn new(check_interval: Duration, failure_threshold: u32) -> Arc<Self> {
        Arc::new(Self {
            statuses: Arc::new(RwLock::new(HashMap::new())),
            check_interval,
            failure_threshold,
        })
    }

    pub fn start(self: Arc<Self>) {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(self.check_interval).await;
                self.check_all().await;
            }
        });
    }

    async fn check_all(&self) {
        let nodes = crate::Discovery::scan().await;
        
        for node in nodes {
            let status = self.check_node(&node.name).await;
            self.statuses.write().await.insert(node.name.clone(), status);
        }
    }

    async fn check_node(&self, cell_name: &str) -> HealthStatus {
        let start = Instant::now();
        
        match self.ping_cell(cell_name).await {
            Ok(_) => HealthStatus {
                is_healthy: true,
                last_check: Instant::now(),
                consecutive_failures: 0,
                latency: start.elapsed(),
            },
            Err(_) => {
                let prev = self.statuses.read().await.get(cell_name).cloned();
                let failures = prev.map(|s| s.consecutive_failures + 1).unwrap_or(1);
                
                HealthStatus {
                    is_healthy: failures < self.failure_threshold,
                    last_check: Instant::now(),
                    consecutive_failures: failures,
                    latency: Duration::from_secs(999),
                }
            }
        }
    }

    async fn ping_cell(&self, cell_name: &str) -> anyhow::Result<()> {
        // For Local:
        let socket_dir = crate::resolve_socket_dir();
        let path = socket_dir.join(format!("{}.sock", cell_name));
        if path.exists() {
             if crate::local::probe_unix_socket(&path).await.is_some() {
                 return Ok(());
             }
        }
        
        // For LAN:
        if let Some(sig) = crate::lan::LanDiscovery::global().find_any(cell_name).await {
             // Simple TCP Connect check to the IP/Port
             if tokio::net::TcpStream::connect(format!("{}:{}", sig.ip, sig.port)).await.is_ok() {
                 return Ok(());
             }
        }

        anyhow::bail!("Ping failed")
    }

    pub async fn is_healthy(&self, cell_name: &str) -> bool {
        self.statuses.read().await
            .get(cell_name)
            .map(|s| s.is_healthy)
            .unwrap_or(false)
    }

    pub async fn get_healthy_nodes(&self) -> Vec<String> {
        self.statuses.read().await
            .iter()
            .filter(|(_, s)| s.is_healthy)
            .map(|(name, _)| name.clone())
            .collect()
    }
}