pub mod vesicle;

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;
use vesicle::Vesicle;

// Async Connection Pool
lazy_static::lazy_static! {
    static ref CONNECTION_POOL: Mutex<HashMap<String, UnixStream>> = Mutex::new(HashMap::new());
}
pub struct Membrane;

impl Membrane {
    /// Fully Async Bind
    pub async fn bind<F, Fut>(signal_def: &str, handler: F) -> Result<()>
    where
        F: Fn(Vesicle) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<Vesicle>> + Send,
    {
        let socket_path =
            std::env::var("CELL_SOCKET_PATH").unwrap_or_else(|_| "run/cell.sock".to_string());
        let path = Path::new(&socket_path);

        if let Some(p) = path.parent() {
            tokio::fs::create_dir_all(p).await?;
        }
        if path.exists() {
            tokio::fs::remove_file(path).await?;
        }

        let listener = UnixListener::bind(path)?;
        let genome_trait = Arc::new(signal_def.as_bytes().to_vec());
        let handler = Arc::new(handler);

        loop {
            match listener.accept().await {
                Ok((mut stream, _)) => {
                    let h = handler.clone();
                    let g = genome_trait.clone();
                    // Spawn a lightweight Tokio Task instead of a Thread
                    tokio::spawn(async move {
                        let _ = handle_transport(&mut stream, &g, &*h).await;
                    });
                }
                Err(e) => eprintln!("Connection error: {}", e),
            }
        }
    }
}

pub struct Synapse {
    stream: UnixStream,
    target: String,
}

impl Synapse {
    pub async fn grow(target_cell: &str) -> Result<Self> {
        // 1. Try Check Pool
        let mut pool = CONNECTION_POOL.lock().await;
        if let Some(stream) = pool.remove(target_cell) {
            return Ok(Self {
                stream,
                target: target_cell.to_string(),
            });
        }
        drop(pool);

        // 2. Connect New
        let golgi_path =
            std::env::var("CELL_GOLGI_SOCK").unwrap_or_else(|_| "run/golgi.sock".to_string());
        let mut stream = UnixStream::connect(&golgi_path)
            .await
            .context("Golgi unreachable")?;

        // 3. Handshake
        stream.write_u8(0x01).await?; // Op: Connect
        let name_bytes = target_cell.as_bytes();
        stream.write_u32(name_bytes.len() as u32).await?;
        stream.write_all(name_bytes).await?;

        let ack = stream.read_u8().await?;
        if ack != 0x00 {
            anyhow::bail!("Golgi rejected connection");
        }

        Ok(Self {
            stream,
            target: target_cell.to_string(),
        })
    }

    pub async fn fire(mut self, vesicle: Vesicle) -> Result<Vesicle> {
        if let Err(e) = self.write_vesicle(&vesicle).await {
            return Err(e);
        }
        let response = read_vesicle(&mut self.stream).await?;

        // Return to pool
        let mut pool = CONNECTION_POOL.lock().await;
        pool.insert(self.target.clone(), self.stream);

        Ok(response)
    }

    async fn write_vesicle(&mut self, v: &Vesicle) -> Result<()> {
        self.stream.write_u32(v.len() as u32).await?;
        self.stream.write_all(v.as_slice()).await?;
        self.stream.flush().await?;
        Ok(())
    }
}

async fn handle_transport<F, Fut>(stream: &mut UnixStream, genome: &[u8], handler: &F) -> Result<()>
where
    F: Fn(Vesicle) -> Fut,
    Fut: std::future::Future<Output = Result<Vesicle>>,
{
    loop {
        let incoming = match read_vesicle(stream).await {
            Ok(v) => v,
            Err(_) => break,
        };

        if incoming.as_slice() == b"__GENOME__" {
            let v_out = Vesicle::wrap(genome.to_vec());
            send_vesicle(stream, v_out).await?;
            continue;
        }

        match handler(incoming).await {
            Ok(resp) => send_vesicle(stream, resp).await?,
            Err(_) => break,
        }
    }
    Ok(())
}

async fn read_vesicle(stream: &mut UnixStream) -> Result<Vesicle> {
    let len = stream.read_u32().await? as usize;
    let mut v = Vesicle::with_capacity(len);
    stream.read_exact(v.as_mut_slice()).await?;
    Ok(v)
}

async fn send_vesicle(stream: &mut UnixStream, v: Vesicle) -> Result<()> {
    stream.write_u32(v.len() as u32).await?;
    stream.write_all(v.as_slice()).await?;
    Ok(())
}
