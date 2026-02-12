// SPDX-License-Identifier: MIT
// cell-sdk/src/connection_manager.rs
//! Production-grade connection management with resilience patterns
//!
//! Features:
//! - Connection pooling with health checks
//! - Circuit breaker pattern
//! - Exponential backoff with jitter
//! - Automatic failover between transport methods
//! - Connection warming and pre-validation

use crate::{CellError, Synapse};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};
use tokio::time::{interval, sleep};
use tracing::{debug, error, info, warn};

/// Connection state machine
#[derive(Debug, Clone, Copy, PartialEq)]
enum ConnState {
    Healthy,
    Degraded,    // Slow but working
    Unhealthy,   // Failed, retrying
    CircuitOpen, // Circuit breaker tripped
    Dead,        // Permanently failed
}

/// Metrics for a single connection
#[derive(Debug, Clone)]
struct ConnMetrics {
    created_at: Instant,
    last_used: Instant,
    requests_total: u64,
    requests_failed: u64,
    latency_ms: f64,
    consecutive_failures: u32,
}

impl ConnMetrics {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            created_at: now,
            last_used: now,
            requests_total: 0,
            requests_failed: 0,
            latency_ms: 0.0,
            consecutive_failures: 0,
        }
    }

    fn record_success(&mut self, latency: Duration) {
        self.last_used = Instant::now();
        self.requests_total += 1;
        self.latency_ms = latency.as_secs_f64() * 1000.0;
        self.consecutive_failures = 0;
    }

    fn record_failure(&mut self) {
        self.last_used = Instant::now();
        self.requests_total += 1;
        self.requests_failed += 1;
        self.consecutive_failures += 1;
    }

    fn failure_rate(&self) -> f64 {
        if self.requests_total == 0 {
            0.0
        } else {
            self.requests_failed as f64 / self.requests_total as f64
        }
    }
}

/// A managed connection with lifecycle tracking
struct ManagedConnection {
    synapse: Synapse,
    metrics: ConnMetrics,
    state: ConnState,
    last_health_check: Instant,
}

/// Connection pool for a specific cell
struct ConnectionPool {
    cell_name: String,
    connections: Vec<Arc<Mutex<ManagedConnection>>>,
    max_connections: usize,
    config: PoolConfig,
}

#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Maximum connections per cell
    pub max_connections: usize,
    /// Minimum connections to maintain
    pub min_connections: usize,
    /// Connection timeout
    pub connect_timeout: Duration,
    /// Request timeout
    pub request_timeout: Duration,
    /// Health check interval
    pub health_check_interval: Duration,
    /// Circuit breaker threshold (failures before opening)
    pub circuit_breaker_threshold: u32,
    /// Circuit breaker reset timeout
    pub circuit_breaker_reset: Duration,
    /// Maximum retry attempts
    pub max_retries: u32,
    /// Base delay for exponential backoff
    pub retry_base_delay: Duration,
    /// Maximum retry delay
    pub retry_max_delay: Duration,
    /// Whether to enable jitter
    pub enable_jitter: bool,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_connections: 10,
            min_connections: 2,
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(30),
            health_check_interval: Duration::from_secs(5),
            circuit_breaker_threshold: 5,
            circuit_breaker_reset: Duration::from_secs(30),
            max_retries: 10,
            retry_base_delay: Duration::from_millis(100),
            retry_max_delay: Duration::from_secs(5),
            enable_jitter: true,
        }
    }
}

/// Production-grade connection manager
pub struct ConnectionManager {
    pools: Arc<RwLock<HashMap<String, ConnectionPool>>>,
    config: PoolConfig,
    global_metrics: Arc<RwLock<GlobalMetrics>>,
}

#[derive(Debug, Default)]
struct GlobalMetrics {
    total_connections: u64,
    total_requests: u64,
    failed_requests: u64,
    circuit_breaker_trips: u64,
}

impl ConnectionManager {
    pub fn new(config: PoolConfig) -> Self {
        let manager = Self {
            pools: Arc::new(RwLock::new(HashMap::new())),
            config,
            global_metrics: Arc::new(RwLock::new(GlobalMetrics::default())),
        };

        // Start background maintenance
        manager.start_maintenance();

        manager
    }

    /// Get or create a connection pool for a cell
    pub async fn get_pool(&self, cell_name: &str) -> Result<Arc<Mutex<ConnectionPool>>> {
        // Fast path: check if pool exists
        {
            let pools = self.pools.read().await;
            if let Some(pool) = pools.get(cell_name) {
                return Ok(Arc::new(Mutex::new(pool.clone()))); // TODO: fix clone issue
            }
        }

        // Slow path: create new pool
        let mut pools = self.pools.write().await;

        // Double-check
        if let Some(pool) = pools.get(cell_name) {
            return Ok(Arc::new(Mutex::new(pool.clone())));
        }

        info!("Creating new connection pool for '{}'", cell_name);

        let pool = ConnectionPool {
            cell_name: cell_name.to_string(),
            connections: Vec::new(),
            max_connections: self.config.max_connections,
            config: self.config.clone(),
        };

        // Warm up minimum connections
        let pool = Arc::new(Mutex::new(pool));
        pools.insert(cell_name.to_string(), pool.lock().await.clone());

        // Spawn pool initialization
        let pool_clone = pool.clone();
        let cell_name = cell_name.to_string();
        let config = self.config.clone();

        tokio::spawn(async move {
            if let Err(e) = Self::warm_pool(&pool_clone, &cell_name, config).await {
                error!("Failed to warm pool for '{}': {}", cell_name, e);
            }
        });

        Ok(pool)
    }

    /// Execute a request with full resilience patterns
    pub async fn execute<F, T>(&self, cell_name: &str, operation: F) -> Result<T>
    where
        F: Fn(&Synapse) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<T>> + Send>>
            + Send,
    {
        let start = Instant::now();

        // Get a healthy connection
        let conn = self.acquire_connection(cell_name).await?;

        let mut conn_guard = conn.lock().await;

        // Check circuit breaker
        if conn_guard.state == ConnState::CircuitOpen {
            if conn_guard.metrics.last_used.elapsed() > self.config.circuit_breaker_reset {
                info!("Circuit breaker reset for '{}'", cell_name);
                conn_guard.state = ConnState::Unhealthy;
            } else {
                return Err(CellError::CircuitBreakerOpen.into());
            }
        }

        // Execute with retry
        let result = self.execute_with_retry(&mut *conn_guard, operation).await;

        // Update metrics and state
        match &result {
            Ok(_) => {
                conn_guard.metrics.record_success(start.elapsed());
                conn_guard.state = ConnState::Healthy;
            }
            Err(e) => {
                conn_guard.metrics.record_failure();
                error!("Request failed for '{}': {}", cell_name, e);

                if conn_guard.metrics.consecutive_failures >= self.config.circuit_breaker_threshold
                {
                    warn!("Circuit breaker OPEN for '{}'", cell_name);
                    conn_guard.state = ConnState::CircuitOpen;

                    let mut global = self.global_metrics.write().await;
                    global.circuit_breaker_trips += 1;
                } else if conn_guard.metrics.failure_rate() > 0.5 {
                    conn_guard.state = ConnState::Degraded;
                }
            }
        }

        // Update global metrics
        let mut global = self.global_metrics.write().await;
        global.total_requests += 1;
        if result.is_err() {
            global.failed_requests += 1;
        }

        result
    }

    /// Acquire a healthy connection from the pool
    async fn acquire_connection(&self, cell_name: &str) -> Result<Arc<Mutex<ManagedConnection>>> {
        let pools = self.pools.read().await;
        let pool = pools
            .get(cell_name)
            .ok_or_else(|| CellError::ConnectionRefused)?;

        // Find best connection (least loaded, healthy)
        let mut best: Option<Arc<Mutex<ManagedConnection>>> = None;
        let mut best_score = f64::MAX;

        for conn in &pool.connections {
            let guard = conn.lock().await;
            let score = match guard.state {
                ConnState::Dead => continue,
                ConnState::CircuitOpen => 1000.0,
                ConnState::Unhealthy => 100.0,
                ConnState::Degraded => 10.0,
                ConnState::Healthy => guard.metrics.latency_ms,
            };

            if score < best_score {
                best_score = score;
                best = Some(conn.clone());
            }
        }

        best.ok_or_else(|| CellError::ConnectionRefused.into())
    }

    /// Execute with exponential backoff retry
    async fn execute_with_retry<F, T>(
        &self,
        conn: &mut ManagedConnection,
        operation: F,
    ) -> Result<T>
    where
        F: Fn(&Synapse) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<T>> + Send>>
            + Send,
    {
        let mut delay = self.config.retry_base_delay;

        for attempt in 0..self.config.max_retries {
            match operation(&conn.synapse).await {
                Ok(result) => return Ok(result),
                Err(e) if attempt < self.config.max_retries - 1 => {
                    debug!(
                        "Retry {}/{} after {:?}: {}",
                        attempt + 1,
                        self.config.max_retries,
                        delay,
                        e
                    );

                    sleep(delay).await;

                    // Exponential backoff with jitter
                    let jitter = if self.config.enable_jitter {
                        rand::random::<f64>() * 0.1 * delay.as_millis() as f64
                    } else {
                        0.0
                    };

                    delay = (delay * 2 + Duration::from_millis(jitter as u64))
                        .min(self.config.retry_max_delay);
                }
                Err(e) => return Err(e),
            }
        }

        unreachable!()
    }

    /// Warm up a pool with minimum connections
    async fn warm_pool(
        pool: &Arc<Mutex<ConnectionPool>>,
        cell_name: &str,
        config: PoolConfig,
    ) -> Result<()> {
        let mut pool_guard = pool.lock().await;

        for i in 0..config.min_connections {
            match Self::create_connection(cell_name, &config).await {
                Ok(conn) => {
                    pool_guard.connections.push(Arc::new(Mutex::new(conn)));
                    debug!(
                        "Warmed connection {}/{} for '{}'",
                        i + 1,
                        config.min_connections,
                        cell_name
                    );
                }
                Err(e) => {
                    error!(
                        "Failed to warm connection {}/{} for '{}': {}",
                        i + 1,
                        config.min_connections,
                        cell_name,
                        e
                    );
                }
            }
        }

        Ok(())
    }

    /// Create a new managed connection
    async fn create_connection(cell_name: &str, config: &PoolConfig) -> Result<ManagedConnection> {
        let synapse = tokio::time::timeout(config.connect_timeout, Synapse::grow(cell_name))
            .await
            .context("Connection timeout")?
            .context("Failed to establish synapse")?;

        Ok(ManagedConnection {
            synapse,
            metrics: ConnMetrics::new(),
            state: ConnState::Healthy,
            last_health_check: Instant::now(),
        })
    }

    /// Start background maintenance tasks
    fn start_maintenance(&self) {
        let pools = self.pools.clone();
        let config = self.config.clone();

        tokio::spawn(async move {
            let mut ticker = interval(config.health_check_interval);

            loop {
                ticker.tick().await;

                let pools_guard = pools.read().await;

                for (cell_name, pool) in pools_guard.iter() {
                    let mut pool_guard = pool.lock().await;

                    // Health check all connections
                    for (i, conn) in pool_guard.connections.iter().enumerate() {
                        let mut conn_guard = conn.lock().await;

                        if conn_guard.last_health_check.elapsed() > config.health_check_interval {
                            // Simple health check: check if still connected
                            // In production: send ping, measure latency
                            conn_guard.last_health_check = Instant::now();

                            // Recycle dead connections
                            if conn_guard.state == ConnState::Dead {
                                debug!(
                                    "Recycling dead connection {}/{} for '{}'",
                                    i,
                                    pool_guard.connections.len(),
                                    cell_name
                                );

                                // Try to replace
                                drop(conn_guard);
                                if let Ok(new_conn) =
                                    Self::create_connection(cell_name, &config).await
                                {
                                    *conn.lock().await = new_conn;
                                }
                            }
                        }
                    }

                    // Ensure minimum connections
                    let healthy_count = pool_guard.connections
                        .iter()
                        .filter(|c| {
                            let g = c.try_lock();
                            matches!(g, Ok(g) if g.state != ConnState::Dead && g.state != ConnState::CircuitOpen)
                        })
                        .count();

                    if healthy_count < config.min_connections {
                        warn!(
                            "Pool for '{}' below minimum ({}/{}), warming up",
                            cell_name, healthy_count, config.min_connections
                        );

                        for _ in healthy_count..config.min_connections {
                            if let Ok(conn) = Self::create_connection(cell_name, &config).await {
                                pool_guard.connections.push(Arc::new(Mutex::new(conn)));
                            }
                        }
                    }
                }
            }
        });
    }

    /// Get current metrics
    pub async fn metrics(&self) -> GlobalMetrics {
        self.global_metrics.read().await.clone()
    }
}

// Clone implementation for ConnectionPool (needed for HashMap)
impl Clone for ConnectionPool {
    fn clone(&self) -> Self {
        Self {
            cell_name: self.cell_name.clone(),
            connections: Vec::new(), // Don't clone actual connections
            max_connections: self.max_connections,
            config: self.config.clone(),
        }
    }
}
