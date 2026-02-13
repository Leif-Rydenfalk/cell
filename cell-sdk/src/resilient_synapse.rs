// SPDX-License-Identifier: MIT
// cell-sdk/src/resilient_synapse.rs
//! Production-grade resilient connection with automatic reconnection
//!
//! Features:
//! - Automatic reconnection with exponential backoff
//! - Transport fallback (SHM → Socket → IO Cell)
//! - Circuit breaker pattern
//! - Health checking and failover

use crate::io_client::IoClient;
use crate::response::Response;
use crate::shm::ShmClient;
use anyhow::{Context, Result};
use cell_core::{channel, VesicleHeader};
use cell_model::rkyv::ser::serializers::AllocSerializer;
use rkyv::Serialize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info, warn};

/// Connection state for health tracking
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConnState {
    Healthy,
    Degraded,     // Slow but working
    Unhealthy,    // Failed, will retry
    Reconnecting, // Currently reconnecting
    CircuitOpen,  // Too many failures, cooling down
}

/// Transport layer abstraction with fallback support
pub enum Transport {
    // Shared memory (fastest, local only)
    Shm {
        client: ShmClient,
        health: Arc<RwLock<ConnState>>,
    },
    // Unix socket (reliable, universal)
    Socket {
        stream: Arc<Mutex<UnixStream>>,
        health: Arc<RwLock<ConnState>>,
        last_activity: Arc<RwLock<Instant>>,
    },
}

// Manual Debug impl since ShmClient doesn't derive Debug
impl std::fmt::Debug for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Transport::Shm { health, .. } => f
                .debug_struct("Transport::Shm")
                .field("health", &health)
                .finish(),
            Transport::Socket {
                health,
                last_activity,
                ..
            } => f
                .debug_struct("Transport::Socket")
                .field("health", &health)
                .field("last_activity", &last_activity)
                .finish(),
        }
    }
}

/// Connection metrics for observability
#[derive(Debug, Clone)]
pub struct ConnMetrics {
    pub created_at: Instant,
    pub last_success: Instant,
    pub last_failure: Option<Instant>,
    pub requests_total: u64,
    pub requests_failed: u64,
    pub reconnections: u64,
    pub current_state: ConnState,
}

/// Configuration for resilience behavior
#[derive(Debug, Clone)]
pub struct ResilienceConfig {
    /// Max reconnection attempts before giving up
    pub max_reconnect_attempts: u32,
    /// Base delay for exponential backoff
    pub reconnect_base_delay: Duration,
    /// Max delay between reconnection attempts
    pub reconnect_max_delay: Duration,
    /// Circuit breaker threshold (consecutive failures)
    pub circuit_breaker_threshold: u32,
    /// Circuit breaker reset timeout
    pub circuit_breaker_reset: Duration,
    /// Health check interval
    pub health_check_interval: Duration,
    /// Request timeout
    pub request_timeout: Duration,
    /// Enable transport upgrade (socket → SHM)
    pub enable_transport_upgrade: bool,
    /// Enable transport downgrade (SHM → socket on failure)
    pub enable_transport_downgrade: bool,
}

impl Default for ResilienceConfig {
    fn default() -> Self {
        Self {
            max_reconnect_attempts: 10,
            reconnect_base_delay: Duration::from_millis(100),
            reconnect_max_delay: Duration::from_secs(5),
            circuit_breaker_threshold: 5,
            circuit_breaker_reset: Duration::from_secs(30),
            health_check_interval: Duration::from_secs(5),
            request_timeout: Duration::from_secs(30),
            enable_transport_upgrade: true,
            enable_transport_downgrade: true,
        }
    }
}

/// Inner state for the resilient synapse
struct SynapseInner {
    transport: Transport,
    cell_name: String,
    my_id: u64,
    metrics: Arc<RwLock<ConnMetrics>>,
    config: ResilienceConfig,
    // Track consecutive failures for circuit breaker
    consecutive_failures: Arc<RwLock<u32>>,
}

/// Production-grade resilient connection handle
///
/// This synapse automatically:
/// 1. Reconnects on transient failures
/// 2. Falls back to alternative transports
/// 3. Implements circuit breaker pattern
/// 4. Provides health metrics
#[derive(Clone)]
pub struct ResilientSynapse {
    inner: Arc<RwLock<SynapseInner>>,
    // Public metrics access
    pub metrics: Arc<RwLock<ConnMetrics>>,
}

impl ResilientSynapse {
    /// Establish a new resilient connection to a cell
    ///
    /// Tries connection strategies in order:
    /// 1. Direct neighbor link (if available)
    /// 2. IO Cell mediated connection
    /// 3. Global registry lookup
    ///
    /// Then attempts transport upgrade to SHM if available.
    pub async fn grow(cell_name: &str) -> Result<Self> {
        Self::grow_with_config(cell_name, ResilienceConfig::default()).await
    }

    /// Connect with custom resilience configuration
    pub async fn grow_with_config(cell_name: &str, config: ResilienceConfig) -> Result<Self> {
        info!("[ResilientSynapse] Connecting to '{}'...", cell_name);

        let (transport, my_id) = Self::establish_connection(cell_name, &config).await?;

        let metrics = Arc::new(RwLock::new(ConnMetrics {
            created_at: Instant::now(),
            last_success: Instant::now(),
            last_failure: None,
            requests_total: 0,
            requests_failed: 0,
            reconnections: 0,
            current_state: ConnState::Healthy,
        }));

        let inner = SynapseInner {
            transport,
            cell_name: cell_name.to_string(),
            my_id,
            metrics: metrics.clone(),
            config,
            consecutive_failures: Arc::new(RwLock::new(0)),
        };

        let synapse = Self {
            inner: Arc::new(RwLock::new(inner)),
            metrics: metrics.clone(),
        };

        // Start background health checker
        synapse.start_health_checker();

        info!(
            "[ResilientSynapse] Connected to '{}' (id={})",
            cell_name, my_id
        );
        Ok(synapse)
    }

    /// Core connection establishment logic
    async fn establish_connection(
        cell_name: &str,
        config: &ResilienceConfig,
    ) -> Result<(Transport, u64)> {
        // Calculate our node ID
        let cwd = std::env::current_dir()?;
        let my_name = cwd.file_name().unwrap_or_default().to_string_lossy();
        let hash = blake3::hash(my_name.as_bytes());
        let my_id = u64::from_le_bytes(hash.as_bytes()[..8].try_into().unwrap());

        // Try 1: Direct neighbor link (fastest, no IO cell needed)
        let neighbor_result = Self::try_neighbor_link(cell_name).await;
        if let Ok(stream) = neighbor_result {
            info!(
                "[ResilientSynapse] Connected via neighbor link to '{}'",
                cell_name
            );
            let transport = Self::wrap_tokio_socket(stream, config).await?;

            // Try to upgrade to SHM if enabled
            if config.enable_transport_upgrade {
                if let Ok(shm) = Self::try_upgrade_to_shm(&transport, cell_name).await {
                    return Ok((shm, my_id));
                }
            }
            return Ok((transport, my_id));
        }

        // Try 2: IO Cell mediated connection
        let io_result = IoClient::connect(cell_name).await;
        if let Ok(stream) = io_result {
            info!(
                "[ResilientSynapse] Connected via IO Cell to '{}'",
                cell_name
            );
            let transport = Self::wrap_socket(stream, config).await?;

            // Try to upgrade to SHM if enabled
            if config.enable_transport_upgrade {
                if let Ok(shm) = Self::try_upgrade_to_shm(&transport, cell_name).await {
                    return Ok((shm, my_id));
                }
            }
            return Ok((transport, my_id));
        }

        // Try 3: Global registry (last resort)
        let global_result = Self::try_global_registry(cell_name).await;
        if let Ok(stream) = global_result {
            info!(
                "[ResilientSynapse] Connected via global registry to '{}'",
                cell_name
            );
            let transport = Self::wrap_tokio_socket(stream, config).await?;
            return Ok((transport, my_id));
        }

        // All strategies failed
        anyhow::bail!(
            "Failed to connect to '{}' via any method. \
             Tried: neighbor link ({:?}), IO cell ({:?}), global registry ({:?})",
            cell_name,
            neighbor_result.err(),
            io_result.err(),
            global_result.err()
        );
    }

    /// Try to connect via direct neighbor symlink
    async fn try_neighbor_link(cell_name: &str) -> Result<UnixStream> {
        let cwd = std::env::current_dir()?;
        let neighbor_tx = cwd.join(".cell/neighbors").join(cell_name).join("tx");

        if !neighbor_tx.exists() {
            anyhow::bail!("Neighbor link not found: {:?}", neighbor_tx);
        }

        // Check if target exists
        let target = tokio::fs::read_link(&neighbor_tx).await?;
        if !target.exists() {
            anyhow::bail!("Neighbor target socket not ready: {:?}", target);
        }

        // Connect using std stream, then convert to tokio
        let std_stream = std::os::unix::net::UnixStream::connect(&neighbor_tx)
            .with_context(|| format!("Failed to connect to neighbor at {:?}", neighbor_tx))?;
        std_stream.set_nonblocking(true)?;

        UnixStream::from_std(std_stream).context("Failed to convert to tokio stream")
    }

    /// Try to connect via global registry in ~/.cell/io/
    async fn try_global_registry(cell_name: &str) -> Result<UnixStream> {
        let home = dirs::home_dir().context("No HOME directory")?;
        let global_sock = home.join(".cell/io").join(format!("{}.sock", cell_name));

        if !global_sock.exists() {
            anyhow::bail!("Global registry socket not found: {:?}", global_sock);
        }

        // Connect using std stream, then convert to tokio
        let std_stream = std::os::unix::net::UnixStream::connect(&global_sock)
            .with_context(|| format!("Failed to connect to global socket at {:?}", global_sock))?;
        std_stream.set_nonblocking(true)?;

        UnixStream::from_std(std_stream).context("Failed to convert to tokio stream")
    }

    /// Wrap a std socket (from IoClient) into our Transport abstraction
    async fn wrap_socket(
        stream: std::os::unix::net::UnixStream,
        _config: &ResilienceConfig,
    ) -> Result<Transport> {
        stream.set_nonblocking(true)?;
        let tokio_stream = UnixStream::from_std(stream)?;
        Self::wrap_tokio_socket(tokio_stream, _config).await
    }

    /// Wrap a tokio socket (from neighbor link or global registry) into our Transport abstraction
    async fn wrap_tokio_socket(
        stream: UnixStream,
        _config: &ResilienceConfig,
    ) -> Result<Transport> {
        Ok(Transport::Socket {
            stream: Arc::new(Mutex::new(stream)),
            health: Arc::new(RwLock::new(ConnState::Healthy)),
            last_activity: Arc::new(RwLock::new(Instant::now())),
        })
    }

    /// Attempt to upgrade socket connection to SHM transport
    async fn try_upgrade_to_shm(transport: &Transport, cell_name: &str) -> Result<Transport> {
        // Only upgrade from socket
        let socket_arc = match transport {
            Transport::Socket { stream, .. } => stream.clone(),
            Transport::Shm { .. } => return Err(anyhow::anyhow!("Already using SHM")),
        };

        let mut stream = socket_arc.lock().await;

        // Send upgrade request
        let payload = b"UPGRADE:SHM";
        let len = payload.len() as u32;

        stream.write_all(&(24 + 1 + 4 + len).to_le_bytes()).await?;
        let header = [0u8; 24];
        stream.write_all(&header).await?;
        stream.write_u8(cell_core::channel::ROUTING).await?;
        stream.write_all(&len.to_le_bytes()).await?;
        stream.write_all(payload).await?;

        // Wait for response with timeout
        let mut len_buf = [0u8; 4];
        match tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut len_buf)).await {
            Ok(Ok(_)) => {
                let resp_len = u32::from_le_bytes(len_buf) as usize;
                let mut resp_buf = vec![0u8; resp_len];
                stream.read_exact(&mut resp_buf).await?;

                // Check if upgrade accepted
                if resp_buf.starts_with(b"UPGRADE:ACK") {
                    info!(
                        "[ResilientSynapse] SHM upgrade accepted for '{}'",
                        cell_name
                    );

                    // Create bidirectional SHM channel
                    let (tx_fd, rx_fd, _, _) = crate::shm::create_shm_channel(32 * 1024 * 1024)?;
                    let shm_client = unsafe { ShmClient::from_fds(tx_fd, rx_fd)? };

                    return Ok(Transport::Shm {
                        client: shm_client,
                        health: Arc::new(RwLock::new(ConnState::Healthy)),
                    });
                }
            }
            _ => {}
        }

        Err(anyhow::anyhow!("SHM upgrade failed or rejected"))
    }

    /// Start background health checking task
    fn start_health_checker(&self) {
        let inner = self.inner.clone();
        let metrics = self.metrics.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));

            loop {
                interval.tick().await;

                let guard = inner.read().await;
                let config = guard.config.clone();
                drop(guard);

                // Check if circuit breaker should reset
                let needs_reconnect = {
                    let m = metrics.read().await;
                    if let Some(last_failure) = m.last_failure {
                        if last_failure.elapsed() > config.circuit_breaker_reset
                            && m.current_state == ConnState::CircuitOpen
                        {
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                };

                if needs_reconnect {
                    info!("[ResilientSynapse] Circuit breaker reset, attempting reconnect");
                    let mut guard = inner.write().await;
                    guard.metrics.write().await.current_state = ConnState::Reconnecting;

                    if let Err(e) = Self::reconnect(&mut guard).await {
                        warn!("[ResilientSynapse] Reconnect failed: {}", e);
                    }
                }
            }
        });
    }

    /// Attempt to reconnect with exponential backoff
    async fn reconnect(inner: &mut SynapseInner) -> Result<()> {
        let cell_name = inner.cell_name.clone();
        let config = inner.config.clone();

        info!("[ResilientSynapse] Reconnecting to '{}'...", cell_name);

        let mut delay = config.reconnect_base_delay;

        for attempt in 1..=config.max_reconnect_attempts {
            match Self::establish_connection(&cell_name, &config).await {
                Ok((new_transport, _)) => {
                    info!(
                        "[ResilientSynapse] Reconnected to '{}' after {} attempts",
                        cell_name, attempt
                    );

                    inner.transport = new_transport;
                    inner.metrics.write().await.reconnections += 1;
                    inner.metrics.write().await.current_state = ConnState::Healthy;
                    *inner.consecutive_failures.write().await = 0;

                    return Ok(());
                }
                Err(e) => {
                    warn!(
                        "[ResilientSynapse] Reconnect attempt {}/{} failed: {}",
                        attempt, config.max_reconnect_attempts, e
                    );

                    if attempt < config.max_reconnect_attempts {
                        tokio::time::sleep(delay).await;
                        delay = (delay * 2).min(config.reconnect_max_delay);
                    }
                }
            }
        }

        // All attempts failed
        inner.metrics.write().await.current_state = ConnState::CircuitOpen;
        anyhow::bail!(
            "Failed to reconnect to '{}' after {} attempts",
            cell_name,
            config.max_reconnect_attempts
        );
    }

    /// Send a request with automatic retry and reconnection
    ///
    /// This is the main API - it handles:
    /// - Serialization
    /// - Transport selection (SHM vs Socket)
    /// - Retry on transient failures
    /// - Automatic reconnection on persistent failures
    /// - Transport downgrade (SHM → Socket) if SHM fails
    pub async fn fire<'a, Req>(&self, request: &Req) -> Result<Response<'a, Vec<u8>>>
    where
        Req: Serialize<AllocSerializer<1024>>,
    {
        let req_bytes = rkyv::to_bytes::<_, 1024>(request)
            .map_err(|e| anyhow::anyhow!("Serialization failed: {}", e))?
            .into_vec();

        // Fast path: try current transport
        match self.try_send(&req_bytes).await {
            Ok(resp) => {
                self.record_success().await;
                return Ok(resp);
            }
            Err(e) => {
                let error_str = e.to_string();
                let is_transient = error_str.contains("Broken pipe")
                    || error_str.contains("Connection reset")
                    || error_str.contains("Timeout");

                if is_transient {
                    warn!(
                        "[ResilientSynapse] Transient failure: {}, attempting recovery",
                        e
                    );
                    self.record_failure().await;

                    // Try recovery
                    return self.fire_with_recovery(&req_bytes).await;
                } else {
                    // Permanent failure
                    return Err(e);
                }
            }
        }
    }

    /// Attempt to send with recovery strategies
    async fn fire_with_recovery<'a>(&self, req_bytes: &[u8]) -> Result<Response<'a, Vec<u8>>> {
        let inner = self.inner.read().await;
        let config = inner.config.clone();
        let cell_name = inner.cell_name.clone();
        drop(inner);

        // Strategy 1: Retry with same transport (transient glitch)
        let mut delay = config.reconnect_base_delay;
        for attempt in 1..=3 {
            tokio::time::sleep(delay).await;

            match self.try_send(req_bytes).await {
                Ok(resp) => {
                    info!("[ResilientSynapse] Recovered after {} retries", attempt);
                    self.record_success().await;
                    return Ok(resp);
                }
                Err(_) => {
                    delay = (delay * 2).min(config.reconnect_max_delay);
                }
            }
        }

        // Strategy 2: Try transport downgrade (SHM → Socket)
        if config.enable_transport_downgrade {
            let inner_guard = self.inner.read().await;
            let is_shm = matches!(inner_guard.transport, Transport::Shm { .. });
            drop(inner_guard);

            if is_shm {
                warn!("[ResilientSynapse] SHM failed, downgrading to socket");

                // Force reconnection with socket only
                let mut inner_guard = self.inner.write().await;
                inner_guard.config.enable_transport_upgrade = false; // Disable SHM for this reconnect

                if let Err(e) = Self::reconnect(&mut inner_guard).await {
                    warn!("[ResilientSynapse] Socket downgrade failed: {}", e);
                } else {
                    // Try send with new socket transport
                    drop(inner_guard);
                    match self.try_send(req_bytes).await {
                        Ok(resp) => {
                            info!("[ResilientSynapse] Downgrade to socket successful");
                            self.record_success().await;
                            return Ok(resp);
                        }
                        Err(e) => warn!("[ResilientSynapse] Socket send failed: {}", e),
                    }
                }
            }
        }

        // Strategy 3: Full reconnection
        info!(
            "[ResilientSynapse] Attempting full reconnection to '{}'",
            cell_name
        );
        {
            let mut inner_guard = self.inner.write().await;
            if let Err(e) = Self::reconnect(&mut inner_guard).await {
                return Err(anyhow::anyhow!("Reconnection failed: {}", e));
            }
        }

        // Final attempt after reconnection
        match self.try_send(req_bytes).await {
            Ok(resp) => {
                info!("[ResilientSynapse] Request succeeded after reconnection");
                self.record_success().await;
                Ok(resp)
            }
            Err(e) => {
                error!(
                    "[ResilientSynapse] Request failed even after reconnection: {}",
                    e
                );
                self.record_failure().await;
                Err(e)
            }
        }
    }

    /// Low-level send attempt on current transport
    async fn try_send<'a>(&self, req_bytes: &[u8]) -> Result<Response<'a, Vec<u8>>> {
        let inner = self.inner.read().await;

        match &inner.transport {
            Transport::Shm { client, health } => {
                // Check health
                let state = *health.read().await;
                if state == ConnState::CircuitOpen {
                    return Err(anyhow::anyhow!("Circuit breaker open"));
                }

                // Send via SHM
                match tokio::time::timeout(
                    inner.config.request_timeout,
                    client.request_raw(req_bytes, channel::APP),
                )
                .await
                {
                    Ok(Ok(msg)) => Ok(Response::Owned(msg.get_bytes().to_vec())),
                    Ok(Err(e)) => {
                        *health.write().await = ConnState::Unhealthy;
                        Err(e.into())
                    }
                    Err(_) => {
                        *health.write().await = ConnState::Unhealthy;
                        Err(anyhow::anyhow!("SHM request timeout"))
                    }
                }
            }
            Transport::Socket {
                stream,
                health,
                last_activity,
            } => {
                // Check health
                let state = *health.read().await;
                if state == ConnState::CircuitOpen {
                    return Err(anyhow::anyhow!("Circuit breaker open"));
                }

                let mut guard = stream.lock().await;
                let result = Self::send_socket(
                    &mut guard,
                    inner.my_id,
                    req_bytes,
                    inner.config.request_timeout,
                )
                .await;

                drop(guard);

                match result {
                    Ok(resp) => {
                        *last_activity.write().await = Instant::now();
                        Ok(resp)
                    }
                    Err(e) => {
                        *health.write().await = ConnState::Unhealthy;
                        Err(e)
                    }
                }
            }
        }
    }

    /// Send over socket transport
    async fn send_socket(
        stream: &mut UnixStream,
        my_id: u64,
        payload: &[u8],
        timeout: Duration,
    ) -> Result<Response<'static, Vec<u8>>> {
        let header = VesicleHeader {
            target_id: 0,
            source_id: my_id,
            ttl: 64,
            flags: 0,
            _pad: [0; 6],
        };

        let total_len = 24 + 1 + payload.len();

        // Send with timeout
        tokio::time::timeout(timeout, async {
            stream.write_all(&(total_len as u32).to_le_bytes()).await?;
            let h_bytes: [u8; 24] = unsafe { std::mem::transmute(header) };
            stream.write_all(&h_bytes).await?;
            stream.write_u8(channel::APP).await?;
            stream.write_all(payload).await?;
            stream.flush().await?;
            Ok::<(), anyhow::Error>(())
        })
        .await??;

        // Receive with timeout - FIXED: handle Result properly
        let mut len_buf = [0u8; 4];
        let read_result = tokio::time::timeout(timeout, stream.read_exact(&mut len_buf)).await;
        match read_result {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => return Err(e.into()),
            Err(_) => return Err(anyhow::anyhow!("Socket read timeout")),
        }

        let len = u32::from_le_bytes(len_buf) as usize;
        if len > 100 * 1024 * 1024 {
            // 100MB sanity limit
            return Err(anyhow::anyhow!("Response too large: {} bytes", len));
        }

        let mut buf = vec![0u8; len];
        let read_result = tokio::time::timeout(timeout, stream.read_exact(&mut buf)).await;
        match read_result {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => return Err(e.into()),
            Err(_) => return Err(anyhow::anyhow!("Socket read timeout")),
        }

        Ok(Response::Owned(buf))
    }

    /// Record successful request
    async fn record_success(&self) {
        let mut m = self.metrics.write().await;
        m.requests_total += 1;
        m.last_success = Instant::now();
        m.current_state = ConnState::Healthy;
        drop(m);

        // Reset consecutive failures
        let inner = self.inner.read().await;
        *inner.consecutive_failures.write().await = 0;
    }

    /// Record failed request
    async fn record_failure(&self) {
        let mut m = self.metrics.write().await;
        m.requests_total += 1;
        m.requests_failed += 1;
        m.last_failure = Some(Instant::now());
        drop(m);

        let inner = self.inner.read().await;
        let mut failures = inner.consecutive_failures.write().await;
        *failures += 1;

        // Check circuit breaker
        if *failures >= inner.config.circuit_breaker_threshold {
            warn!(
                "[ResilientSynapse] Circuit breaker OPEN for '{}'",
                inner.cell_name
            );
            drop(failures);

            let mut m = self.metrics.write().await;
            m.current_state = ConnState::CircuitOpen;

            // Mark transport as unhealthy
            match &inner.transport {
                Transport::Shm { health, .. } => {
                    *health.write().await = ConnState::CircuitOpen;
                }
                Transport::Socket { health, .. } => {
                    *health.write().await = ConnState::CircuitOpen;
                }
            }
        }
    }

    /// Get current connection state
    pub async fn state(&self) -> ConnState {
        self.metrics.read().await.current_state
    }

    /// Force reconnection (useful for explicit recovery)
    pub async fn force_reconnect(&self) -> Result<()> {
        let mut inner = self.inner.write().await;
        Self::reconnect(&mut inner).await
    }
}

// Implement std::fmt::Debug for ResilientSynapse
impl std::fmt::Debug for ResilientSynapse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResilientSynapse")
            .field("metrics", &self.metrics)
            .finish()
    }
}
