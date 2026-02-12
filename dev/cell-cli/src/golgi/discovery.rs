use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;

#[derive(Serialize, Deserialize, Debug)]
pub enum Signal {
    Heartbeat { id: String, port: u16 }, // "I am here, this is my QUIC port"
    Lookup { target: String },           // "Where is 'worker'?"
    Location { target: String, addr: SocketAddr }, // Response
}

pub struct Discovery {
    socket: Arc<UdpSocket>,
    lighthouse_addr: SocketAddr,
}

impl Discovery {
    pub async fn new(bind_port: u16, lighthouse: &str) -> Result<Self> {
        // Bind a separate UDP socket for Signaling
        // Note: Real hole punching usually requires sharing the socket,
        // but for MVP we assume the Lighthouse IP lookup is sufficient for NATs that aren't symmetric.
        let socket = UdpSocket::bind(format!("0.0.0.0:{}", bind_port)).await?;
        let lighthouse_addr = lighthouse.parse()?;

        Ok(Self {
            socket: Arc::new(socket),
            lighthouse_addr,
        })
    }

    pub async fn register(&self, my_id: String, my_quic_port: u16) -> Result<()> {
        let msg = Signal::Heartbeat {
            id: my_id,
            port: my_quic_port,
        };
        let bytes = serde_json::to_vec(&msg)?;
        self.socket.send_to(&bytes, self.lighthouse_addr).await?;
        Ok(())
    }

    pub async fn lookup(&self, target: String) -> Result<Option<SocketAddr>> {
        let msg = Signal::Lookup {
            target: target.clone(),
        };
        let bytes = serde_json::to_vec(&msg)?;
        self.socket.send_to(&bytes, self.lighthouse_addr).await?;

        // Quick wait for response
        let mut buf = [0u8; 1024];
        match tokio::time::timeout(
            std::time::Duration::from_secs(2),
            self.socket.recv_from(&mut buf),
        )
        .await
        {
            Ok(Ok((len, _))) => {
                if let Ok(Signal::Location { target: t, addr }) =
                    serde_json::from_slice(&buf[..len])
                {
                    if t == target {
                        return Ok(Some(addr));
                    }
                }
            }
            _ => {}
        }
        Ok(None)
    }

    // For the Lighthouse Server itself
    pub async fn serve(port: u16) -> Result<()> {
        let socket = UdpSocket::bind(format!("0.0.0.0:{}", port)).await?;
        println!("[Lighthouse] Active on port {}", port);

        let mut registry: std::collections::HashMap<String, SocketAddr> =
            std::collections::HashMap::new();
        let mut buf = [0u8; 1024];

        loop {
            let (len, src) = socket.recv_from(&mut buf).await?;
            if let Ok(msg) = serde_json::from_slice::<Signal>(&buf[..len]) {
                match msg {
                    Signal::Heartbeat { id, port } => {
                        // The Peer's public IP is 'src.ip()', but the port is the one they claimed (QUIC port)
                        let public_quic_addr = SocketAddr::new(src.ip(), port);
                        // println!("[Lighthouse] Registered {} at {}", id, public_quic_addr);
                        registry.insert(id, public_quic_addr);
                    }
                    Signal::Lookup { target } => {
                        if let Some(addr) = registry.get(&target) {
                            let resp = Signal::Location {
                                target,
                                addr: *addr,
                            };
                            let resp_bytes = serde_json::to_vec(&resp)?;
                            socket.send_to(&resp_bytes, src).await?;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}
