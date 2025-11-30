use crate::protocol::{MitosisRequest, MitosisResponse, SHM_UPGRADE_ACK, SHM_UPGRADE_REQUEST};
use crate::vesicle::Vesicle;
use anyhow::{bail, Context, Result};
#[cfg(target_os = "linux")]
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

enum Transport {
    /// Placeholder state used during transport switching
    Upgrading,
    Socket(UnixStream),
    #[cfg(target_os = "linux")]
    SharedMemory {
        client: crate::shm::ShmClient,
        _socket: UnixStream,
    },
}

pub struct Synapse {
    transport: Transport,
    upgrade_attempted: bool,
}

impl Synapse {
    pub async fn grow(cell_name: &str) -> Result<Self> {
        let socket_dir = resolve_socket_dir();
        let socket_path = socket_dir.join(format!("{}.sock", cell_name));

        // 1. FAST PATH
        if let Ok(stream) = UnixStream::connect(&socket_path).await {
            return Ok(Self {
                transport: Transport::Socket(stream),
                upgrade_attempted: false,
            });
        }

        // 2. SLOW PATH
        let umbilical_path = resolve_umbilical_path();
        let mut umbilical = UnixStream::connect(&umbilical_path)
            .await
            .with_context(|| {
                format!(
                    "Failed to connect to Umbilical Cord at {:?}. Is the Root running?",
                    umbilical_path
                )
            })?;

        let req = MitosisRequest::Spawn {
            cell_name: cell_name.into(),
        };
        let bytes = crate::rkyv::to_bytes::<_, 256>(&req)
            .map_err(|e| anyhow::anyhow!("Serialization failed: {}", e))?
            .into_vec();

        umbilical
            .write_all(&(bytes.len() as u32).to_le_bytes())
            .await?;
        umbilical.write_all(&bytes).await?;

        let mut len_buf = [0u8; 4];
        umbilical.read_exact(&mut len_buf).await?;
        let len = u32::from_le_bytes(len_buf) as usize;
        let mut buf = vec![0u8; len];
        umbilical.read_exact(&mut buf).await?;

        let resp = crate::rkyv::from_bytes::<MitosisResponse>(&buf)
            .map_err(|e| anyhow::anyhow!("Deserialization failed: {:?}", e))?;

        match resp {
            MitosisResponse::Ok { .. } => {
                for _ in 0..50 {
                    if let Ok(stream) = UnixStream::connect(&socket_path).await {
                        return Ok(Self {
                            transport: Transport::Socket(stream),
                            upgrade_attempted: false,
                        });
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                bail!(
                    "Cell '{}' spawned but failed to bind socket at {:?} in time.",
                    cell_name,
                    socket_path
                );
            }
            MitosisResponse::Denied { reason } => bail!("Mitosis Denied: {}", reason),
        }
    }

    pub async fn fire<
        T: crate::rkyv::Serialize<crate::rkyv::ser::serializers::AllocSerializer<1024>>,
    >(
        &mut self,
        payload: T,
    ) -> Result<Vesicle> {
        let bytes = crate::rkyv::to_bytes::<_, 1024>(&payload)
            .map_err(|e| anyhow::anyhow!("Serialization error: {}", e))?
            .into_vec();

        self.fire_bytes(bytes).await
    }

    pub async fn fire_bytes(&mut self, bytes: Vec<u8>) -> Result<Vesicle> {
        // Try upgrade on first call
        #[cfg(target_os = "linux")]
        if !self.upgrade_attempted {
            self.upgrade_attempted = true;
            if let Err(e) = self.try_upgrade_to_shm().await {
                eprintln!("!! SHM UPGRADE FAILED: {} !! Falling back to socket.", e);
            } else {
                // SPSC upgrade successful
            }
        }

        // Now dispatch based on current transport
        match &mut self.transport {
            Transport::Socket(stream) => {
                stream
                    .write_all(&(bytes.len() as u32).to_le_bytes())
                    .await?;
                stream.write_all(&bytes).await?;

                let mut len_buf = [0u8; 4];
                stream.read_exact(&mut len_buf).await?;
                let len = u32::from_le_bytes(len_buf) as usize;
                let mut buf = vec![0u8; len];
                stream.read_exact(&mut buf).await?;

                Ok(Vesicle::wrap(buf))
            }
            #[cfg(target_os = "linux")]
            Transport::SharedMemory { client, .. } => {
                // Use the optimized SPSC client
                // We provide a closure that writes directly into the shared memory slot
                let resp = client
                    .request(bytes.len(), |mut buf| {
                        buf.copy_from_slice(&bytes);
                    })
                    .await?;
                Ok(Vesicle::wrap(resp))
            }

            Transport::Upgrading => bail!("Transport is currently upgrading"),
        }
    }

    #[cfg(target_os = "linux")]
    async fn try_upgrade_to_shm_internal(&mut self) -> Result<crate::shm::ShmClient> {
        if let Transport::Socket(stream) = &mut self.transport {
            stream
                .write_all(&(SHM_UPGRADE_REQUEST.len() as u32).to_le_bytes())
                .await?;
            stream.write_all(SHM_UPGRADE_REQUEST).await?;

            let mut len_buf = [0u8; 4];
            stream.read_exact(&mut len_buf).await?;
            let len = u32::from_le_bytes(len_buf) as usize;
            let mut buf = vec![0u8; len];
            stream.read_exact(&mut buf).await?;

            if buf != SHM_UPGRADE_ACK {
                bail!("Server rejected SHM upgrade");
            }

            stream.readable().await?;
            let fds = crate::shm::GapJunction::recv_fds(stream.as_raw_fd())?;
            if fds.len() != 2 {
                bail!("Expected 2 FDs, got {}", fds.len());
            }

            // Client attaches to the ring buffers created by the server.
            // fds[0]: Client TX (Server RX)
            // fds[1]: Client RX (Server TX)
            let tx = unsafe { crate::shm::GapJunction::attach(fds[0])? };
            let rx = unsafe { crate::shm::GapJunction::attach(fds[1])? };

            return Ok(crate::shm::ShmClient::new(tx, rx));
        }
        bail!("Not socket transport");
    }

    #[cfg(target_os = "linux")]
    async fn try_upgrade_to_shm(&mut self) -> Result<()> {
        let client = self.try_upgrade_to_shm_internal().await?;

        let old_transport = std::mem::replace(&mut self.transport, Transport::Upgrading);

        if let Transport::Socket(stream) = old_transport {
            self.transport = Transport::SharedMemory {
                client,
                _socket: stream,
            };
            Ok(())
        } else {
            self.transport = old_transport;
            bail!("Transport was not Socket during upgrade")
        }
    }
}

fn resolve_socket_dir() -> PathBuf {
    if let Ok(p) = std::env::var("CELL_SOCKET_DIR") {
        return PathBuf::from(p);
    }
    let container_socket_dir = std::path::Path::new("/tmp/cell");
    let container_umbilical = std::path::Path::new("/tmp/mitosis.sock");
    if container_socket_dir.exists() && container_umbilical.exists() {
        return container_socket_dir.to_path_buf();
    }
    if let Some(home) = dirs::home_dir() {
        return home.join(".cell/run");
    }
    PathBuf::from("/tmp/cell")
}

fn resolve_umbilical_path() -> PathBuf {
    if let Ok(p) = std::env::var("CELL_UMBILICAL") {
        return PathBuf::from(p);
    }
    let container_cord = std::path::Path::new("/tmp/mitosis.sock");
    if container_cord.exists() {
        return container_cord.to_path_buf();
    }
    if let Some(home) = dirs::home_dir() {
        return home.join(".cell/run/mitosis.sock");
    }
    PathBuf::from("/tmp/mitosis.sock")
}
