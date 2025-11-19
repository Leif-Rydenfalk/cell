use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UnixListener, UnixStream};

pub struct Router {
    socket_path: PathBuf,
    tcp_bind: Option<String>,
    routes: HashMap<String, RouteTarget>,
}

#[derive(Clone, Debug)]
pub enum RouteTarget {
    LocalUnix(PathBuf),
    RemoteTcp(String),
}

impl Router {
    pub fn new(run_dir: &std::path::Path, tcp_bind: Option<String>) -> Self {
        Self {
            socket_path: run_dir.join("router.sock"),
            tcp_bind,
            routes: HashMap::new(),
        }
    }

    pub fn add_route(&mut self, name: String, target: RouteTarget) {
        self.routes.insert(name, target);
    }

    pub async fn serve(self) -> Result<()> {
        // 1. Setup Local Unix Listener
        if self.socket_path.exists() {
            tokio::fs::remove_file(&self.socket_path).await.ok();
        }
        let unix_listener = UnixListener::bind(&self.socket_path).context("Bind Unix")?;

        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&self.socket_path, std::fs::Permissions::from_mode(0o777))?;

        // 2. Setup Remote TCP Listener (Optional)
        let tcp_listener = if let Some(addr) = &self.tcp_bind {
            println!("🌐 Router listening on TCP: {}", addr);
            Some(TcpListener::bind(addr).await.context("Bind TCP")?)
        } else {
            None
        };

        let routes = std::sync::Arc::new(self.routes);

        // 3. Event Loop (Handle both Unix and TCP)
        loop {
            tokio::select! {
                // Local Cell connecting to Router
                res = unix_listener.accept() => {
                    if let Ok((stream, _)) = res {
                        let r = routes.clone();
                        tokio::spawn(async move {
                            let _ = handle_generic_connection(stream, r).await;
                        });
                    }
                }
                // Remote Router connecting to us
                res = accept_tcp_option(tcp_listener.as_ref()) => {
                    if let Some(Ok((stream, _))) = res {
                        let r = routes.clone();
                        tokio::spawn(async move {
                            // TCP Nodelay is crucial for low latency "Ping" style messages
                            let _ = stream.set_nodelay(true);
                            let _ = handle_generic_connection(stream, r).await;
                        });
                    }
                }
            }
        }
    }
}

// Helper to make tokio::select! work with Option<TcpListener>
async fn accept_tcp_option(
    listener: Option<&TcpListener>,
) -> Option<std::io::Result<(TcpStream, std::net::SocketAddr)>> {
    match listener {
        Some(l) => Some(l.accept().await),
        None => std::future::pending().await,
    }
}

// Generic Handler: Works for UnixStream AND TcpStream
async fn handle_generic_connection<S>(
    mut client: S,
    routes: std::sync::Arc<HashMap<String, RouteTarget>>,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    // 1. Read OpCode
    let mut op = [0u8; 1];
    if client.read_exact(&mut op).await.is_err() {
        return Ok(());
    }

    match op[0] {
        0x01 => {
            // CONNECT OpCode
            // 2. Read Service Name
            let mut len_buf = [0u8; 4];
            client.read_exact(&mut len_buf).await?;
            let len = u32::from_be_bytes(len_buf) as usize;

            let mut name_buf = vec![0u8; len];
            client.read_exact(&mut name_buf).await?;
            let name = String::from_utf8(name_buf).unwrap_or_default();

            // 3. Routing Logic
            match routes.get(&name) {
                Some(target) => {
                    // Send ACK (0x00) to the Client (or the calling Remote Router)
                    client.write_all(&[0x00]).await?;

                    match target {
                        RouteTarget::LocalUnix(path) => {
                            // Bridge to Local Service
                            let target_stream = UnixStream::connect(path)
                                .await
                                .context("Target dead locally")?;
                            bridge(client, target_stream).await?;
                        }
                        RouteTarget::RemoteTcp(addr) => {
                            // Bridge to Remote Router
                            // Note: We are proxying. We must perform the handshake with the remote router too!
                            let mut target_stream = TcpStream::connect(addr)
                                .await
                                .context("Target dead remotely")?;
                            target_stream.set_nodelay(true)?;

                            // REPLAY HANDSHAKE to the next hop
                            target_stream.write_all(&[0x01]).await?; // OpCode
                            target_stream.write_all(&(len as u32).to_be_bytes()).await?; // Len
                            target_stream.write_all(name.as_bytes()).await?; // Name

                            // Wait for Remote ACK
                            let mut ack = [0u8; 1];
                            target_stream.read_exact(&mut ack).await?;
                            if ack[0] != 0x00 {
                                // Remote refused
                                return Ok(());
                            }

                            bridge(client, target_stream).await?;
                        }
                    }
                }
                None => {
                    // Send Error (0xFF)
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

async fn bridge<A, B>(a: A, b: B) -> Result<()>
where
    A: AsyncRead + AsyncWrite + Unpin,
    B: AsyncRead + AsyncWrite + Unpin,
{
    let (ra, wa) = tokio::io::split(a);
    let (rb, wb) = tokio::io::split(b);

    let _ = tokio::try_join!(pipe(ra, wb), pipe(rb, wa));
    Ok(())
}

async fn pipe<R, W>(mut reader: R, mut writer: W) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    // 1MB Buffer for Max Throughput
    let mut buf = vec![0u8; 1024 * 1024];
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
