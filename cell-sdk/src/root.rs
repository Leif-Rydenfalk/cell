use crate::container::{build_spawn_cmd, Runtime};
use crate::protocol::{MitosisRequest, MitosisResponse};
use crate::vesicle::Vesicle;
use anyhow::Result;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

pub struct MyceliumRoot {
    socket_dir: PathBuf,
    dna_path: PathBuf,
    umbilical_path: PathBuf,
}

impl MyceliumRoot {
    /// Ignites the Mycelium. call this at the top of main().
    pub async fn ignite() -> Result<Self> {
        let socket_dir = PathBuf::from("/tmp/cell");
        tokio::fs::create_dir_all(&socket_dir).await?;

        // In v0.3, we assume binaries are in the same folder as the Root executable
        let dna_path = std::env::current_exe()?.parent().unwrap().to_path_buf();
        let umbilical_path = socket_dir.join("mitosis.sock");

        // Clean up old cord
        if umbilical_path.exists() {
            tokio::fs::remove_file(&umbilical_path).await?;
        }

        let listener = UnixListener::bind(&umbilical_path)?;

        let root = Self {
            socket_dir,
            dna_path,
            umbilical_path,
        };

        // Spawn the Listener Task
        let r = root.clone();
        tokio::spawn(async move {
            loop {
                if let Ok((stream, _)) = listener.accept().await {
                    let mut r_inner = r.clone();
                    tokio::spawn(async move {
                        let _ = r_inner.handle_child(stream).await;
                    });
                }
            }
        });

        Ok(root)
    }

    // Allow cloning for the async task
    fn clone(&self) -> Self {
        Self {
            socket_dir: self.socket_dir.clone(),
            dna_path: self.dna_path.clone(),
            umbilical_path: self.umbilical_path.clone(),
        }
    }

    async fn handle_child(&mut self, mut stream: UnixStream) -> Result<()> {
        // 1. Read Request
        // (Using simple framing for brevity, use full rkyv in prod)
        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).await?;
        let len = u32::from_le_bytes(len_buf) as usize;
        let mut buf = vec![0u8; len];
        stream.read_exact(&mut buf).await?;

        let req = crate::rkyv::from_bytes::<MitosisRequest>(&buf)
            .map_err(|_| anyhow::anyhow!("Invalid Mitosis Protocol"))?;

        match req {
            MitosisRequest::Spawn { cell_name } => {
                println!("[ROOT] Umbilical Request: Spawn '{}'", cell_name);

                // 2. Spawn Logic
                // Detect if we want containers or raw based on env (default raw for ease)
                let runtime = if std::env::var("CELL_CONTAINER").is_ok() {
                    Runtime::Podman
                } else {
                    Runtime::Raw
                };

                let mut cmd = build_spawn_cmd(
                    runtime,
                    &cell_name,
                    &self.dna_path,
                    &self.socket_dir,
                    &self.umbilical_path,
                )?;

                match cmd.spawn() {
                    Ok(_) => {
                        let resp = MitosisResponse::Ok {
                            socket_path: format!("/tmp/cell/{}.sock", cell_name),
                        };
                        self.send_resp(&mut stream, resp).await?;
                    }
                    Err(e) => {
                        self.send_resp(
                            &mut stream,
                            MitosisResponse::Denied {
                                reason: e.to_string(),
                            },
                        )
                        .await?;
                    }
                }
            }
        }
        Ok(())
    }

    async fn send_resp(&self, stream: &mut UnixStream, resp: MitosisResponse) -> Result<()> {
        let bytes = crate::rkyv::to_bytes::<_, 256>(&resp)?.into_vec();
        stream
            .write_all(&(bytes.len() as u32).to_le_bytes())
            .await?;
        stream.write_all(&bytes).await?;
        Ok(())
    }
}
