// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use crate::resolve_socket_dir;
use crate::response::Response;
use crate::transport::UnixTransport;
#[cfg(feature = "shm")]
use crate::transport::ShmTransport;
#[cfg(feature = "shm")]
use crate::shm::{RingBuffer, ShmClient};

use crate::retry::RetryPolicy;
use crate::circuit_breaker::{CircuitBreaker};
use crate::deadline::Deadline;

use cell_core::{Transport, TransportError, channel};
use cell_model::bridge::{BridgeRequest, BridgeResponse};
use anyhow::{bail, Result, Context};
use rkyv::ser::serializers::AllocSerializer;
use rkyv::{Archive, Serialize};
use std::sync::Arc;
use tokio::net::UnixStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{info, warn, debug};
use std::time::Duration;

#[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
use cell_model::protocol::{SHM_UPGRADE_ACK, SHM_UPGRADE_REQUEST};
#[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
use std::os::unix::io::AsRawFd;

pub struct SynapseConfig {
    pub retry_policy: RetryPolicy,
    pub circuit_breaker_threshold: u64,
    pub circuit_breaker_timeout: Duration,
    pub default_timeout: Duration,
}

impl Default for SynapseConfig {
    fn default() -> Self {
        Self {
            retry_policy: RetryPolicy::default(),
            circuit_breaker_threshold: 5,
            circuit_breaker_timeout: Duration::from_secs(30),
            default_timeout: Duration::from_secs(5),
        }
    }
}

pub struct Synapse {
    cell_name: String,
    transport: Box<dyn Transport>,
    
    retry_policy: RetryPolicy,
    circuit_breaker: Arc<CircuitBreaker>,
    default_deadline: Deadline,
    
    #[cfg(feature = "shm")]
    shm_client: Option<ShmClient>,
}

impl Synapse {
    /// Connect to a cell.
    /// Supports:
    /// 1. Local sockets ("ledger")
    /// 2. URI Schemes ("mavlink:drone", "quic:remote") via Gateways
    /// 3. Fallback to default gateway ("axon")
    pub async fn grow(connection_string: &str) -> Result<Self> {
        Self::grow_with_config(connection_string, SynapseConfig::default()).await
    }

    pub async fn grow_with_config(connection_string: &str, config: SynapseConfig) -> Result<Self> {
        info!("[Synapse] Connecting to '{}'...", connection_string);

        // 1. Parse Connection String
        let (gateway_name, target) = Self::resolve_gateway(connection_string);

        // 2. Try Direct Connection (Fast Path)
        // If it's a simple name like "ledger", check local socket first
        if gateway_name.is_none() {
            if let Ok(syn) = Self::connect_local(target, &config).await {
                return Ok(syn);
            }
        }

        // 3. Routing via Gateway (Slow Path)
        // If we have a scheme (e.g. "mavlink") or local connect failed, ask the gateway.
        let gateway = gateway_name.unwrap_or("axon"); // Default to Axon
        
        debug!("[Synapse] Routing '{}' via gateway '{}'", target, gateway);

        // Recursive: Connect to the gateway itself (must be local)
        // We assume the gateway itself is reachable locally.
        let mut gateway_syn = match Self::connect_local(gateway, &SynapseConfig::default()).await {
            Ok(g) => g,
            Err(_) => bail!("Target '{}' not found and gateway '{}' is not running.", target, gateway),
        };

        // Handshake: Ask gateway to mount the target
        let req = BridgeRequest::Mount { target: target.to_string() };
        let req_bytes = rkyv::to_bytes::<_, 256>(&req)?.into_vec();
        
        // We send this on the APP channel of the gateway
        let resp_wrapper = gateway_syn.fire_on_channel(channel::APP, &req_bytes).await?;
        
        let resp_bytes = match resp_wrapper {
            Response::Owned(v) => v,
            Response::Borrowed(v) => v.to_vec(),
            _ => bail!("Invalid response from gateway"),
        };

        let response = cell_model::rkyv::check_archived_root::<BridgeResponse>(&resp_bytes)
            .map_err(|e| anyhow::anyhow!("Gateway protocol mismatch: {}", e))?
            .deserialize(&mut rkyv::Infallible).unwrap();

        match response {
            BridgeResponse::Mounted { socket_path } => {
                info!("[Synapse] Gateway mounted '{}' at '{}'", target, socket_path);
                // Connect to the proxy socket provided by the gateway
                // Note: We bypass `connect_local` helpers to connect to arbitrary path
                Self::connect_to_path(&socket_path, target, &config).await
            }
            BridgeResponse::NotFound => bail!("Gateway '{}' could not find target '{}'", gateway, target),
            BridgeResponse::Error { message } => bail!("Gateway error: {}", message),
        }
    }

    /// Determines the gateway cell based on URI scheme
    fn resolve_gateway(conn_str: &str) -> (Option<&str>, &str) {
        if let Some((scheme, rest)) = conn_str.split_once(':') {
            // Map schemes to convention-based cell names
            let gateway = match scheme {
                "quic" => "axon",
                "tcp" => "tcp-gateway",
                "mavlink" => "mavlink-gateway",
                "ssh" => "ssh-gateway",
                other => other, // e.g. "custom:target" -> "custom" cell
            };
            (Some(gateway), rest)
        } else {
            (None, conn_str)
        }
    }

    async fn connect_local(cell_name: &str, config: &SynapseConfig) -> Result<Self> {
        let socket_dir = resolve_socket_dir();
        let socket_path = socket_dir.join(format!("{}.sock", cell_name));
        let path_str = socket_path.to_string_lossy().to_string();
        Self::connect_to_path(&path_str, cell_name, config).await
    }

    async fn connect_to_path(path: &str, cell_name: &str, config: &SynapseConfig) -> Result<Self> {
        let mut transport: Option<Box<dyn Transport>> = None;
        let mut shm_client: Option<ShmClient> = None;

        if let Ok(mut stream) = UnixStream::connect(path).await {
            
            #[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
            if std::env::var("CELL_DISABLE_SHM").is_err() {
                match Self::try_upgrade_to_shm(&mut stream).await {
                    Ok(client) => {
                        transport = Some(Box::new(ShmTransport::new(
                                ShmClient::new(client.tx.clone(), client.rx.clone())
                        )));
                        shm_client = Some(client);
                    }
                    Err(_) => {
                        // Fallback to Unix if upgrade rejected (common for proxies)
                    }
                }
            }
            
            if transport.is_none() {
                // If upgrade consumed stream, we need a new one or carefully handle buffer.
                // Assuming upgrade logic handles clean stream state or we reconnect.
                // For simplicity, we reconnect if upgrade failed to ensure clean state.
                if let Ok(clean_stream) = UnixStream::connect(path).await {
                    transport = Some(Box::new(UnixTransport::new(clean_stream)));
                }
            }
        }

        if let Some(t) = transport {
            Ok(Self {
                cell_name: cell_name.to_string(),
                transport: t,
                retry_policy: config.retry_policy.clone(),
                circuit_breaker: CircuitBreaker::new(
                    config.circuit_breaker_threshold,
                    config.circuit_breaker_timeout,
                ),
                default_deadline: Deadline::new(config.default_timeout),
                #[cfg(feature = "shm")]
                shm_client,
            })
        } else {
            bail!("Failed to connect to socket: {}", path);
        }
    }
    
    #[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
    async fn try_upgrade_to_shm(stream: &mut UnixStream) -> Result<ShmClient> {
        let cred = stream.peer_cred()?;
        let my_uid = nix::unistd::getuid().as_raw();
        if cred.uid() != my_uid { bail!("UID mismatch"); }

        let mut frame = Vec::with_capacity(1 + SHM_UPGRADE_REQUEST.len());
        frame.push(0x00); 
        frame.extend_from_slice(SHM_UPGRADE_REQUEST);

        stream.write_all(&(frame.len() as u32).to_le_bytes()).await?;
        stream.write_all(&frame).await?;

        let mut challenge = [0u8; 32];
        stream.read_exact(&mut challenge).await?;
        
        let auth_token = crate::membrane::get_shm_auth_token()?;
        let response = blake3::hash(&[&challenge, auth_token.as_slice()].concat());
        stream.write_all(response.as_bytes()).await?;

        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).await?;
        let len = u32::from_le_bytes(len_buf) as usize;
        let mut ack = vec![0u8; len];
        stream.read_exact(&mut ack).await?;

        if ack != SHM_UPGRADE_ACK { bail!("SHM Upgrade Rejected"); }

        stream.readable().await?;
        let fds = Self::recv_fds(stream.as_raw_fd())?;
        if fds.len() != 2 { bail!("Expected 2 FDs"); }

        let tx = unsafe { RingBuffer::attach(fds[0])? };
        let rx = unsafe { RingBuffer::attach(fds[1])? };

        Ok(ShmClient::new(tx, rx))
    }

    #[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
    fn recv_fds(socket_fd: std::os::unix::io::RawFd) -> Result<Vec<std::os::unix::io::RawFd>> {
        use nix::sys::socket::{recvmsg, ControlMessageOwned, MsgFlags};
        use std::io::IoSliceMut;

        let mut dummy = [0u8; 1];
        let mut iov = [IoSliceMut::new(&mut dummy)];
        let mut cmsg_buf = nix::cmsg_space!([std::os::unix::io::RawFd; 4]);

        let msg = recvmsg::<()>(socket_fd, &mut iov, Some(&mut cmsg_buf), MsgFlags::empty())?;

        let mut fds = Vec::new();
        for cmsg in msg.cmsgs() {
            if let ControlMessageOwned::ScmRights(received_fds) = cmsg {
                fds.extend(received_fds);
            }
        }
        Ok(fds)
    }

    async fn heal(&mut self) -> Result<()> {
        warn!("[Synapse] Connection lost to '{}'. Healing...", self.cell_name);
        
        const RECONNECT_ATTEMPTS: usize = 5;
        const BASE_DELAY: u64 = 200;

        for i in 0..RECONNECT_ATTEMPTS {
            tokio::time::sleep(Duration::from_millis(BASE_DELAY * (1 << i))).await;
            
            // Retry the original connection string logic implicitly?
            // Synapse struct stores `cell_name`. If it was "mavlink:drone", we need to re-resolve.
            // Simplified: We attempt to reconnect using the stored name.
            match Synapse::grow(&self.cell_name).await {
                Ok(new_syn) => {
                    info!("[Synapse] Reconnected to '{}'.", self.cell_name);
                    self.transport = new_syn.transport;
                    #[cfg(feature = "shm")]
                    { self.shm_client = new_syn.shm_client; }
                    self.circuit_breaker = new_syn.circuit_breaker;
                    return Ok(());
                },
                Err(e) => {
                    warn!("[Synapse] Reconnect attempt {}/{} failed: {}", i+1, RECONNECT_ATTEMPTS, e);
                }
            }
        }
        bail!("Failed to heal connection to '{}' after {} attempts", self.cell_name, RECONNECT_ATTEMPTS);
    }

    async fn call_transport(&mut self, data: &[u8]) -> Result<Vec<u8>> {
         match self.transport.call(data).await {
             Ok(resp) => Ok(resp),
             Err(e) => {
                 match e {
                     TransportError::Io | TransportError::ConnectionClosed | TransportError::Timeout => {
                         if let Err(heal_err) = self.heal().await {
                             return Err(anyhow::anyhow!("Transport Error: {:?}, Healing Failed: {}", e, heal_err));
                         }
                         self.transport.call(data).await.map_err(|e2| anyhow::anyhow!("Retry after heal failed: {:?}", e2))
                     },
                     _ => Err(anyhow::anyhow!("Transport Error: {:?}", e)),
                 }
             }
         }
    }

    pub async fn fire_on_channel(&mut self, channel_id: u8, data: &[u8]) -> Result<Response<Vec<u8>>> {
        if self.circuit_breaker.is_open() {
            return Err(anyhow::anyhow!("Circuit breaker open for '{}'", self.cell_name));
        }

        #[cfg(feature = "shm")]
        if let Some(client) = &self.shm_client {
             if let Ok(msg) = client.request_raw(data, channel_id).await {
                 return Ok(Response::Owned(msg.get_bytes().to_vec()));
             }
        }

        let data_vec = data.to_vec();
        let mut attempt = 0;
        let mut delay = self.retry_policy.base_delay;
        
        loop {
            attempt += 1;
            
            let result = self.default_deadline.execute({
                let data_ref = &data_vec;
                let breaker = self.circuit_breaker.clone();
                
                async move {
                   breaker.call(|| { Ok(()) }).map_err(|e| anyhow::anyhow!("{}", e))?; 
                   let mut frame = Vec::with_capacity(1 + data_ref.len());
                   frame.push(channel_id);
                   frame.extend_from_slice(data_ref);
                   Ok(frame)
                }
            }).await;

            let frame = match result {
                Ok(f) => f,
                Err(e) => {
                    if attempt >= self.retry_policy.max_attempts { return Err(e); }
                    tokio::time::sleep(delay).await;
                    delay = std::cmp::min(Duration::from_secs_f64(delay.as_secs_f64() * self.retry_policy.multiplier), self.retry_policy.max_delay);
                    continue;
                }
            };
            
            match self.call_transport(&frame).await {
                Ok(resp_bytes) => return Ok(Response::Owned(resp_bytes)),
                Err(e) => {
                    if attempt >= self.retry_policy.max_attempts { return Err(e); }
                    tokio::time::sleep(delay).await;
                    delay = std::cmp::min(Duration::from_secs_f64(delay.as_secs_f64() * self.retry_policy.multiplier), self.retry_policy.max_delay);
                }
            }
        }
    }

    pub async fn fire<'a, Req, Resp>(&'a mut self, request: &Req) -> Result<Response<'a, Resp>>
    where
        Req: Serialize<AllocSerializer<1024>>,
        Resp: Archive + 'a,
        Resp::Archived: rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'static>> + 'static,
    {
        let req_bytes = match rkyv::to_bytes::<_, 1024>(request) {
            Ok(b) => b.into_vec(),
            Err(e) => return Err(anyhow::anyhow!("Serialization error: {}", e)),
        };

        #[cfg(feature = "shm")]
        if let Some(client) = &self.shm_client {
             if let Ok(msg) = client.request::<Req, Resp>(request, channel::APP).await {
                 return Ok(Response::ZeroCopy(msg));
             }
        }

        match self.fire_on_channel(channel::APP, &req_bytes).await? {
            Response::Owned(vec) => Ok(Response::Owned(vec)),
            _ => Err(anyhow::anyhow!("Unexpected response type")),
        }
    }
}