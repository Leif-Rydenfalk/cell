use anyhow::{Context, Result};
use fd_lock::RwLock;
use std::fs::File;
use std::path::{Path, PathBuf};
use tokio::net::{UnixListener, UnixStream};

pub struct Membrane;

impl Membrane {
    pub async fn bind<F, Fut>(name: &str, handler: F) -> Result<()>
    where
        F: Fn(crate::vesicle::Vesicle) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<crate::vesicle::Vesicle>> + Send,
    {
        let socket_dir = std::env::var("CELL_SOCKET_DIR").unwrap_or("/tmp/cell".into());
        let dir = Path::new(&socket_dir);
        tokio::fs::create_dir_all(dir).await?;

        // 1. SINGULARITY CHECK (File Locking)
        // If this blocks or fails, another instance is running.
        let lock_path = dir.join(format!("{}.lock", name));
        let lock_file = File::create(&lock_path)?;
        let mut _guard = RwLock::new(lock_file);

        // Try to acquire write lock. If fails, we are redundant.
        if _guard.try_write().is_err() {
            println!("[{}] Instance already running. Exiting.", name);
            return Ok(());
        }

        // 2. BIND SOCKET
        let socket_path = dir.join(format!("{}.sock", name));
        if socket_path.exists() {
            tokio::fs::remove_file(&socket_path).await?;
        }
        let listener = UnixListener::bind(&socket_path)?;
        println!("[{}] Membrane Active at {:?}", name, socket_path);

        // 3. EVENT LOOP (With Apoptosis Timer)
        let handler = std::sync::Arc::new(handler);
        let last_active = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs(),
        ));

        // Background Suicide Watch
        let la_clone = last_active.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs();
                let last = la_clone.load(std::sync::atomic::Ordering::Relaxed);

                // If no requests for 60 seconds, die.
                if now - last > 60 {
                    println!("Apoptosis Triggered (Idle).");
                    std::process::exit(0);
                }
            }
        });

        loop {
            match listener.accept().await {
                Ok((mut stream, _)) => {
                    // Update heartbeat
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)?
                        .as_secs();
                    last_active.store(now, std::sync::atomic::Ordering::Relaxed);

                    let h = handler.clone();
                    tokio::spawn(async move {
                        // ... (Protocol handling same as before)
                    });
                }
                Err(_) => break,
            }
        }
        Ok(())
    }
}
