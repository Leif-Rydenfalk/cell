use crate::protocol::GENOME_REQUEST;
use anyhow::{Context, Result};
use fd_lock::RwLock;
use std::fs::File;
use std::path::PathBuf;
use tokio::net::UnixListener;

pub struct Membrane;

impl Membrane {
    /// Bind the membrane to a socket and start the event loop.
    ///
    /// `genome_json`: Optional JSON string representing the CellGenome.
    /// If provided, the Membrane will automatically respond to introspection requests.
    pub async fn bind<F, Fut>(name: &str, handler: F, genome_json: Option<String>) -> Result<()>
    where
        F: Fn(crate::vesicle::Vesicle) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<crate::vesicle::Vesicle>> + Send,
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
        let handler = std::sync::Arc::new(handler);
        let genome = std::sync::Arc::new(genome_json);

        let last_active = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs(),
        ));

        // Apoptosis Timer
        let la_clone = last_active.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs();
                let last = la_clone.load(std::sync::atomic::Ordering::Relaxed);

                if now - last > 300 { // Increased timeout for dev comfort
                     // std::process::exit(0);
                }
            }
        });

        loop {
            match listener.accept().await {
                Ok((mut stream, _)) => {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)?
                        .as_secs();
                    last_active.store(now, std::sync::atomic::Ordering::Relaxed);

                    let h = handler.clone();
                    let g = genome.clone();

                    tokio::spawn(async move {
                        loop {
                            use tokio::io::{AsyncReadExt, AsyncWriteExt};
                            let mut len_buf = [0u8; 4];
                            if stream.read_exact(&mut len_buf).await.is_err() {
                                break;
                            }
                            let len = u32::from_le_bytes(len_buf) as usize;
                            let mut buf = vec![0u8; len];
                            if stream.read_exact(&mut buf).await.is_err() {
                                break;
                            }

                            // --- INTROSPECTION CHECK ---
                            if buf == GENOME_REQUEST {
                                if let Some(json) = g.as_ref() {
                                    let bytes = json.as_bytes();
                                    let _ =
                                        stream.write_all(&(bytes.len() as u32).to_le_bytes()).await;
                                    let _ = stream.write_all(bytes).await;
                                } else {
                                    // Send empty response if no genome
                                    let _ = stream.write_all(&0u32.to_le_bytes()).await;
                                }
                                continue;
                            }
                            // ---------------------------

                            let vesicle = crate::vesicle::Vesicle::wrap(buf);

                            match h(vesicle).await {
                                Ok(resp) => {
                                    if stream
                                        .write_all(&(resp.len() as u32).to_le_bytes())
                                        .await
                                        .is_err()
                                    {
                                        break;
                                    }
                                    if stream.write_all(resp.as_slice()).await.is_err() {
                                        break;
                                    }
                                }
                                Err(e) => {
                                    eprintln!("Handler Error: {}", e);
                                    break;
                                }
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
