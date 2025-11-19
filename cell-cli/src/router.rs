use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
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

        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&self.socket_path, std::fs::Permissions::from_mode(0o777))?;

        let routes = std::sync::Arc::new(self.routes);

        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let r = routes.clone();
                    tokio::spawn(async move {
                        if let Err(_e) = handle_connection(stream, r).await {
                            // eprintln!("Router stream error: {}", _e);
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
    let mut op = [0u8; 1];
    if client.read_exact(&mut op).await.is_err() {
        return Ok(());
    }

    match op[0] {
        0x01 => {
            let mut len_buf = [0u8; 4];
            client.read_exact(&mut len_buf).await?;
            let len = u32::from_be_bytes(len_buf) as usize;

            let mut name_buf = vec![0u8; len];
            client.read_exact(&mut name_buf).await?;
            let name = String::from_utf8(name_buf).unwrap_or_default();

            match routes.get(&name) {
                Some(target) => {
                    client.write_all(&[0x00]).await?;

                    match target {
                        RouteTarget::LocalUnix(path) => {
                            let mut target_stream = UnixStream::connect(path)
                                .await
                                .context("Target dead locally")?;

                            let (ri, wi) = client.split();
                            let (ro, wo) = target_stream.split();

                            let _ = tokio::try_join!(pipe(ri, wo), pipe(ro, wi));
                        }
                        RouteTarget::RemoteTcp(addr) => {
                            let mut target_stream = TcpStream::connect(addr)
                                .await
                                .context("Target dead remotely")?;

                            let (ri, wi) = client.split();
                            let (ro, wo) = target_stream.split();

                            let _ = tokio::try_join!(pipe(ri, wo), pipe(ro, wi));
                        }
                    }
                }
                None => {
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

async fn pipe<R, W>(mut reader: R, mut writer: W) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    // // 1MB (Max Speed, High RAM usage) - Best for Compute Clusters
    // let mut buf = vec![0u8; 1024 * 1024];

    // 64KB (High Speed, Low RAM usage) - Best for General Internet
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n]).await?;
    }
    writer.shutdown().await?;
    Ok(())
}
