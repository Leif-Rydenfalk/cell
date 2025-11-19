use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UnixListener, UnixStream};

pub struct Router {
    socket_path: PathBuf,
    routes: HashMap<String, RouteTarget>,
}

#[derive(Clone, Debug)]
pub enum RouteTarget {
    LocalUnix(PathBuf),
    RemoteTcp(String),
}

impl Router {
    pub fn new(run_dir: &std::path::Path) -> Self {
        Self {
            socket_path: run_dir.join("router.sock"),
            routes: HashMap::new(),
        }
    }

    pub fn add_local_route(&mut self, name: String, path: PathBuf) {
        self.routes.insert(name, RouteTarget::LocalUnix(path));
    }

    pub fn add_tcp_route(&mut self, name: String, address: String) {
        self.routes.insert(name, RouteTarget::RemoteTcp(address));
    }

    pub async fn serve(self) -> Result<()> {
        if self.socket_path.exists() {
            tokio::fs::remove_file(&self.socket_path).await.ok();
        }

        let listener =
            UnixListener::bind(&self.socket_path).context("Failed to bind Router socket")?;

        // Allow access
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&self.socket_path, std::fs::Permissions::from_mode(0o777))?;

        let routes = std::sync::Arc::new(self.routes);

        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let r = routes.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(stream, r).await {
                            // eprintln!("Router stream error: {}", e);
                        }
                    });
                }
                Err(e) => eprintln!("Router accept error: {}", e),
            }
        }
    }
}

async fn handle_connection(
    mut client: UnixStream,
    routes: std::sync::Arc<HashMap<String, RouteTarget>>,
) -> Result<()> {
    // 1. Handshake: OpCode
    let mut op = [0u8; 1];
    if client.read_exact(&mut op).await.is_err() {
        return Ok(());
    }

    match op[0] {
        0x01 => {
            // CONNECT
            // Read Name Len
            let mut len_buf = [0u8; 4];
            client.read_exact(&mut len_buf).await?;
            let len = u32::from_be_bytes(len_buf) as usize;

            // Read Name
            let mut name_buf = vec![0u8; len];
            client.read_exact(&mut name_buf).await?;
            let name = String::from_utf8(name_buf).unwrap_or_default();

            match routes.get(&name) {
                Some(target) => {
                    // Ack (0x00)
                    client.write_all(&[0x00]).await?;

                    match target {
                        RouteTarget::LocalUnix(path) => {
                            let mut target_stream = UnixStream::connect(path)
                                .await
                                .context("Target dead locally")?;

                            let (mut ri, mut wi) = client.split();
                            let (mut ro, mut wo) = target_stream.split();
                            let _ = tokio::try_join!(
                                tokio::io::copy(&mut ri, &mut wo),
                                tokio::io::copy(&mut ro, &mut wi)
                            );
                        }
                        RouteTarget::RemoteTcp(addr) => {
                            let mut target_stream = TcpStream::connect(addr)
                                .await
                                .context("Target dead remotely")?;

                            let (mut ri, mut wi) = client.split();
                            let (mut ro, mut wo) = target_stream.split();
                            let _ = tokio::try_join!(
                                tokio::io::copy(&mut ri, &mut wo),
                                tokio::io::copy(&mut ro, &mut wi)
                            );
                        }
                    }
                }
                None => {
                    // Fail (0xFF)
                    let _ = client.write_all(&[0xFF]).await;
                }
            }
        }
        _ => {
            let _ = client.write_all(&[0xFF]).await;
        }
    }
    Ok(())
}
