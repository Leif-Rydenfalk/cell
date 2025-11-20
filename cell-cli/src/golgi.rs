use crate::antigens::Antigens;
use crate::synapse;
use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UnixListener, UnixStream};

#[derive(Clone, Debug)]
pub enum Target {
    GapJunction(PathBuf),
    Axon(String),
}

pub struct Golgi {
    socket_path: PathBuf,
    axon_bind: Option<String>,
    routes: Arc<HashMap<String, Target>>,
    identity: Arc<Antigens>,
}

fn sys_log(level: &str, msg: &str) {
    let timestamp = humantime::format_rfc3339_seconds(SystemTime::now());
    eprintln!("[{}] [{}] [GOLGI] {}", timestamp, level, msg);
}

impl Golgi {
    pub fn new(
        run_dir: &std::path::Path,
        axon_bind: Option<String>,
        routes_map: HashMap<String, Target>,
    ) -> Result<Self> {
        let identity = Antigens::load_or_create().context("Failed to load node identity.")?;

        sys_log(
            "INFO",
            &format!("Identity Loaded. Node ID: {}", identity.public_key_str),
        );

        Ok(Self {
            socket_path: run_dir.join("golgi.sock"),
            axon_bind,
            routes: Arc::new(routes_map),
            identity: Arc::new(identity),
        })
    }

    pub async fn run(self) -> Result<()> {
        if self.socket_path.exists() {
            tokio::fs::remove_file(&self.socket_path).await.ok();
        }
        let unix_listener = UnixListener::bind(&self.socket_path)
            .context("CRITICAL: Failed to bind internal Gap Junction socket.")?;

        let tcp_listener = if let Some(addr) = &self.axon_bind {
            let l = TcpListener::bind(addr).await.context("Axon bind failed")?;
            sys_log("INFO", &format!("Axon Interface online: {}", addr));
            Some(l)
        } else {
            sys_log("WARN", "Node is isolated (No TCP).");
            None
        };

        let routes = self.routes;
        let identity = self.identity;
        sys_log("INFO", "Transport subsystem active.");

        loop {
            tokio::select! {
                res = unix_listener.accept() => {
                    if let Ok((stream, _)) = res {
                        let r = routes.clone();
                        let i = identity.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_local_signal(stream, r, i).await {
                                sys_log("ERROR", &format!("Local: {}", e));
                            }
                        });
                    }
                }
                res = accept_tcp_optional(tcp_listener.as_ref()) => {
                    if let Some(Ok((stream, addr))) = res {
                        let r = routes.clone();
                        let i = identity.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_remote_signal(stream, r, i).await {
                                sys_log("WARN", &format!("Remote {}: {}", addr, e));
                            }
                        });
                    }
                }
            }
        }
    }
}

async fn handle_local_signal(
    mut stream: UnixStream,
    routes: Arc<HashMap<String, Target>>,
    identity: Arc<Antigens>,
) -> Result<()> {
    let mut op = [0u8; 1];
    if stream.read_exact(&mut op).await.is_err() {
        return Ok(());
    }

    if op[0] == 0x01 {
        // CONNECT
        let target_name = read_len_str(&mut stream).await?;

        match routes.get(&target_name) {
            Some(Target::GapJunction(path)) => {
                match UnixStream::connect(path).await {
                    Ok(target) => {
                        stream.write_all(&[0x00]).await?; // ACK
                        bridge_plain(stream, target).await?;
                    }
                    Err(e) => {
                        sys_log("WARN", &format!("Service '{}' dead: {}", target_name, e));
                        stream.write_all(&[0xFF]).await?; // NACK
                    }
                }
            }
            Some(Target::Axon(addr)) => {
                let tcp_stream = TcpStream::connect(addr).await?;
                tcp_stream.set_nodelay(true)?;
                let (mut secure_stream, _) =
                    synapse::connect_secure(tcp_stream, &identity.keypair, true).await?;

                // Handshake complete. Now send Request inside tunnel.
                {
                    let mut buf = vec![0u8; 1024];
                    let mut payload = vec![0x01]; // OpCode
                    payload.extend(&(target_name.len() as u32).to_be_bytes());
                    payload.extend(target_name.as_bytes());

                    let len = secure_stream
                        .state
                        .write_message(&payload, &mut buf)
                        .unwrap();
                    synapse::write_frame(&mut secure_stream.inner, &buf[..len]).await?;
                }

                // Wait for ACK
                let frame = synapse::read_frame(&mut secure_stream.inner).await?;
                let mut buf = vec![0u8; 1024];
                let len = secure_stream.state.read_message(&frame, &mut buf)?;

                if len > 0 && buf[0] == 0x00 {
                    stream.write_all(&[0x00]).await?;
                    synapse::bridge_secure_to_plain(secure_stream, stream).await?;
                } else {
                    stream.write_all(&[0xFF]).await?;
                }
            }
            None => {
                sys_log("WARN", &format!("Route unknown: {}", target_name));
                stream.write_all(&[0xFF]).await?;
            }
        }
    }
    Ok(())
}

async fn handle_remote_signal(
    stream: TcpStream,
    routes: Arc<HashMap<String, Target>>,
    identity: Arc<Antigens>,
) -> Result<()> {
    stream.set_nodelay(true)?;
    let (mut secure_stream, remote_pub) =
        synapse::connect_secure(stream, &identity.keypair, false).await?;
    let remote_id = B64.encode(remote_pub);

    // Read OpCode
    let frame = synapse::read_frame(&mut secure_stream.inner).await?;
    let mut buf = vec![0u8; 1024];
    let len = secure_stream.state.read_message(&frame, &mut buf)?;

    if len < 5 {
        return Ok(());
    } // Too short

    if buf[0] == 0x01 {
        let name_len = u32::from_be_bytes(buf[1..5].try_into()?) as usize;
        if len < 5 + name_len {
            return Ok(());
        }
        let target_name = String::from_utf8(buf[5..5 + name_len].to_vec())?;

        if let Some(Target::GapJunction(path)) = routes.get(&target_name) {
            match UnixStream::connect(path).await {
                Ok(target) => {
                    // ACK
                    let len = secure_stream
                        .state
                        .write_message(&[0x00], &mut buf)
                        .unwrap();
                    synapse::write_frame(&mut secure_stream.inner, &buf[..len]).await?;
                    synapse::bridge_secure_to_plain(secure_stream, target).await?;
                }
                Err(_) => {
                    // NACK
                    let len = secure_stream
                        .state
                        .write_message(&[0xFF], &mut buf)
                        .unwrap();
                    synapse::write_frame(&mut secure_stream.inner, &buf[..len]).await?;
                }
            }
        } else {
            sys_log(
                "WARN",
                &format!("{} -> Unknown Route {}", remote_id, target_name),
            );
            let len = secure_stream
                .state
                .write_message(&[0xFF], &mut buf)
                .unwrap();
            synapse::write_frame(&mut secure_stream.inner, &buf[..len]).await?;
        }
    }
    Ok(())
}

async fn accept_tcp_optional(
    l: Option<&TcpListener>,
) -> Option<std::io::Result<(TcpStream, std::net::SocketAddr)>> {
    match l {
        Some(l) => Some(l.accept().await),
        None => std::future::pending().await,
    }
}

async fn bridge_plain<A, B>(a: A, b: B) -> Result<()>
where
    A: AsyncReadExt + AsyncWriteExt + Unpin + Send + 'static,
    B: AsyncReadExt + AsyncWriteExt + Unpin + Send + 'static,
{
    let (mut ra, mut wa) = tokio::io::split(a);
    let (mut rb, mut wb) = tokio::io::split(b);
    let _ = tokio::try_join!(
        tokio::io::copy(&mut ra, &mut wb),
        tokio::io::copy(&mut rb, &mut wa)
    );
    Ok(())
}

async fn read_len_str<R: tokio::io::AsyncRead + Unpin>(r: &mut R) -> Result<String> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > 65536 {
        anyhow::bail!("Too long");
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    Ok(String::from_utf8(buf)?)
}
