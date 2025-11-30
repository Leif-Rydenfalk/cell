use crate::protocol::{GENOME_REQUEST, SHM_UPGRADE_REQUEST};
use crate::vesicle::Vesicle;
use anyhow::{Context, Result};
use fd_lock::RwLock;
use std::fs::File;
#[cfg(target_os = "linux")]
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

pub struct Membrane;

impl Membrane {
    /// Bind the membrane to a socket and start the event loop.
    pub async fn bind<F, Fut>(name: &str, handler: F, genome_json: Option<String>) -> Result<()>
    where
        F: Fn(Vesicle) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<Vesicle>> + Send,
    {
        let socket_dir = resolve_socket_dir();
        tokio::fs::create_dir_all(&socket_dir).await?;

        // 1. SINGULARITY CHECK
        let lock_path = socket_dir.join(format!("{}.lock", name));
        let lock_file = File::create(&lock_path).context("Failed to create lock file")?;
        let mut _guard = RwLock::new(lock_file);

        if _guard.try_write().is_err() {
            println!("[{}] Instance already running. Exiting.", name);
            return Ok(());
        }

        // 2. BIND SOCKET
        let socket_path = socket_dir.join(format!("{}.sock", name));
        if socket_path.exists() {
            tokio::fs::remove_file(&socket_path).await?;
        }
        let listener = UnixListener::bind(&socket_path)
            .with_context(|| format!("Failed to bind socket at {:?}", socket_path))?;

        println!("[{}] Membrane Active at {:?}", name, socket_path);

        // 3. EVENT LOOP
        let handler = Arc::new(handler);
        let genome = Arc::new(genome_json);

        loop {
            match listener.accept().await {
                Ok((mut stream, _)) => {
                    let h = handler.clone();
                    let g = genome.clone();
                    let name = name.to_string();

                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(&mut stream, h, g, &name).await {
                            // Don't log unexpected EOF during disconnects
                            if e.to_string() != "early eof" {
                                eprintln!("[{}] Connection Error: {}", name, e);
                            }
                        }
                    });
                }
                Err(_) => break,
            }
        }
        Ok(())
    }
}

async fn handle_connection<F, Fut>(
    stream: &mut UnixStream,
    handler: Arc<F>,
    genome: Arc<Option<String>>,
    cell_name: &str,
) -> Result<()>
where
    F: Fn(Vesicle) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Vesicle>> + Send,
{
    loop {
        // Read Length Prefix
        let mut len_buf = [0u8; 4];
        if stream.read_exact(&mut len_buf).await.is_err() {
            return Ok(()); // Connection closed
        }
        let len = u32::from_le_bytes(len_buf) as usize;

        // Read Payload
        let mut buf = vec![0u8; len];
        if stream.read_exact(&mut buf).await.is_err() {
            return Ok(());
        }

        // --- PROTOCOL DISPATCH ---

        // 1. Introspection
        if buf == GENOME_REQUEST {
            let resp = if let Some(json) = genome.as_ref() {
                json.as_bytes()
            } else {
                &[]
            };
            stream.write_all(&(resp.len() as u32).to_le_bytes()).await?;
            stream.write_all(resp).await?;
            continue;
        }

        // 2. Transport Upgrade (Adaptive Transport)
        if buf == SHM_UPGRADE_REQUEST {
            #[cfg(target_os = "linux")]
            {
                // CREATE SPSC BUFFERS PER CLIENT
                // We name them uniquely to avoid collisions if multiple clients connect
                // though strictly the memfd name is just for debugging.
                let (shm_rx, fd_rx) = crate::shm::GapJunction::forge(&format!("{}_rx", cell_name))?;
                let (shm_tx, fd_tx) = crate::shm::GapJunction::forge(&format!("{}_tx", cell_name))?;

                println!(
                    "[{}] Upgrading connection to SPSC Shared Memory...",
                    cell_name
                );

                // Send FDs: We send [ClientTX (fd_rx), ClientRX (fd_tx)]
                let raw_socket_fd = stream.as_raw_fd();

                // Ack first so client is ready to recvmsg
                let ack = crate::protocol::SHM_UPGRADE_ACK;
                stream.write_all(&(ack.len() as u32).to_le_bytes()).await?;
                stream.write_all(ack).await?;

                // Send FDs
                crate::shm::GapJunction::send_fds(raw_socket_fd, &[fd_rx, fd_tx])?;

                // Adapter: Convert the Zero-Copy Slice -> Owned Vesicle -> Handler -> Owned Vec -> Zero-Copy Write
                // This bridge is necessary because the User Handler API is defined in terms of Vesicle.
                let shm_handler = Arc::new(move |data: &[u8]| {
                    let h = handler.clone();
                    // We must clone the data here because Vesicle takes ownership.
                    // Ideally, Vesicle would support borrowing, but for now we clone.
                    // This is still much faster than socket syscalls.
                    let v = Vesicle::wrap(data.to_vec());
                    async move { h(v).await.map(|resp| resp.as_slice().to_vec()) }
                });

                // Switch loop to SPSC SHM Handler (in shm.rs)
                return crate::shm::handle_shm_loop(shm_rx, shm_tx, shm_handler).await;
            }
            #[cfg(not(target_os = "linux"))]
            {
                // Reject or Ignore
                let empty = b"";
                stream
                    .write_all(&(empty.len() as u32).to_le_bytes())
                    .await?;
                continue;
            }
        }

        // 3. Standard Vesicle (RPC)
        let vesicle = Vesicle::wrap(buf);
        match handler(vesicle).await {
            Ok(resp) => {
                stream.write_all(&(resp.len() as u32).to_le_bytes()).await?;
                stream.write_all(resp.as_slice()).await?;
            }
            Err(e) => {
                eprintln!("Handler Error: {}", e);
                return Err(e);
            }
        }
    }
}

pub fn resolve_socket_dir() -> PathBuf {
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
