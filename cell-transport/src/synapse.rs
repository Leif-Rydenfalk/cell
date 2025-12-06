// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use crate::resolve_socket_dir;
use crate::response::Response;
use crate::transport::UnixTransport;
#[cfg(feature = "axon")]
use crate::transport::QuicTransport;
#[cfg(feature = "shm")]
use crate::transport::ShmTransport;
#[cfg(feature = "shm")]
use crate::shm::{RingBuffer, ShmClient};

use crate::retry::RetryPolicy;
use crate::circuit_breaker::{CircuitBreaker};
use crate::deadline::Deadline;

use cell_core::{Transport, TransportError, channel};
use anyhow::{bail, Result, Context};
use rkyv::ser::serializers::AllocSerializer;
use rkyv::{Archive, Serialize};
use std::sync::Arc;
use tokio::net::UnixStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{info, warn, error};
use std::time::Duration;

#[cfg(feature = "axon")]
use cell_axon::AxonClient;

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
    pub async fn grow(cell_name: &str) -> Result<Self> {
        Self::grow_with_config(cell_name, SynapseConfig::default()).await
    }

    pub async fn grow_with_config(cell_name: &str, config: SynapseConfig) -> Result<Self> {
        info!("[Synapse] Connecting to '{}'...", cell_name);

        let socket_dir = resolve_socket_dir();
        let socket_path = socket_dir.join(format!("{}.sock", cell_name));
        
        let mut transport: Option<Box<dyn Transport>> = None;
        let mut shm_client: Option<ShmClient> = None;

        if let Ok(mut stream) = UnixStream::connect(&socket_path).await {
            info!("[Synapse] ✓ Local connection established");
            
            #[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
            if std::env::var("CELL_DISABLE_SHM").is_err() {
                match Self::try_upgrade_to_shm(&mut stream).await {
                    Ok(client) => {
                        info!("[Synapse] ✓ Upgraded to Shared Memory");
                        transport = Some(Box::new(ShmTransport::new(
                                ShmClient::new(client.tx.clone(), client.rx.clone())
                        )));
                        shm_client = Some(client);
                    }
                    Err(e) => {
                        warn!("[Synapse] SHM Upgrade failed ({}), falling back...", e);
                    }
                }
            }
            
            if transport.is_none() {
                // Re-connect to ensure clean stream state after potential failed upgrade
                if let Ok(clean_stream) = UnixStream::connect(&socket_path).await {
                    transport = Some(Box::new(UnixTransport::new(clean_stream)));
                }
            }
        }

        if transport.is_none() {
            #[cfg(feature = "axon")]
            {
                if let Some(conn) = AxonClient::connect(cell_name).await? {
                    transport = Some(Box::new(QuicTransport::new(conn)));
                }
            }
        }
        
        if let Some(t) = transport {
            Ok(Self {
                cell_name: cell_name.to_string(),
                transport: t,
                retry_policy: config.retry_policy,
                circuit_breaker: CircuitBreaker::new(
                    config.circuit_breaker_threshold,
                    config.circuit_breaker_timeout,
                ),
                default_deadline: Deadline::new(config.default_timeout),
                #[cfg(feature = "shm")]
                shm_client,
            })
        } else {
            bail!("Cell '{}' not found", cell_name);
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
        
        let auth_token = crate::membrane::get_shm_auth_token();
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
            
            match Synapse::grow(&self.cell_name).await {
                Ok(new_syn) => {
                    info!("[Synapse] Reconnected to '{}'.", self.cell_name);
                    self.transport = new_syn.transport;
                    #[cfg(feature = "shm")]
                    { self.shm_client = new_syn.shm_client; }
                    
                    // Also transfer resilience state
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
                 // Try healing once on connection issues
                 match e {
                     TransportError::Io | TransportError::ConnectionClosed | TransportError::Timeout => {
                         if let Err(heal_err) = self.heal().await {
                             return Err(anyhow::anyhow!("Transport Error: {:?}, Healing Failed: {}", e, heal_err));
                         }
                         // Retry once after heal
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
             // SHM is usually robust, but we wrap it too
             // For SHM we skip the standard Retry/CircuitBreaker loop for performance,
             // assuming if SHM exists it is stable locally.
             if let Ok(msg) = client.request_raw(data, channel_id).await {
                 return Ok(Response::Owned(msg.get_bytes().to_vec()));
             }
             // Fallthrough to transport
        }

        let data_vec = data.to_vec();
        
        let mut attempt = 0;
        let mut delay = self.retry_policy.base_delay;
        
        loop {
            attempt += 1;
            
            let result = self.default_deadline.execute({
                let data_ref = &data_vec;
                let breaker = self.circuit_breaker.clone();
                
                // FIX: async move ensures breaker is moved into the Future, extending its lifetime
                async move {
                   breaker.call(|| { Ok(()) }).map_err(|e| anyhow::anyhow!("{}", e))?; // Check breaker
                   
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
            
            // Execute transport call
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
        // Serialization check
        let req_bytes = match rkyv::to_bytes::<_, 1024>(request) {
            Ok(b) => b.into_vec(),
            Err(e) => return Err(anyhow::anyhow!("Serialization error: {}", e)),
        };

        #[cfg(feature = "shm")]
        if let Some(client) = &self.shm_client {
             // Try zero-copy path
             if let Ok(msg) = client.request::<Req, Resp>(request, channel::APP).await {
                 return Ok(Response::ZeroCopy(msg));
             }
        }

        // Delegate to resilient channel fire
        match self.fire_on_channel(channel::APP, &req_bytes).await? {
            Response::Owned(vec) => Ok(Response::Owned(vec)),
            _ => Err(anyhow::anyhow!("Unexpected response type")),
        }
    }
}