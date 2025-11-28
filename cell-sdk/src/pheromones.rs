use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::RwLock;

const MULTICAST_ADDR: Ipv4Addr = Ipv4Addr::new(239, 255, 0, 1);
const PORT: u16 = 9099;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Signal {
    pub cell_name: String,
    pub ip: String,
    pub port: u16,
}

pub struct PheromoneSystem {
    cache: Arc<RwLock<HashMap<String, String>>>, // Name -> "IP:Port"
    socket: Arc<UdpSocket>,
}

impl PheromoneSystem {
    pub async fn ignite() -> Result<Arc<Self>> {
        // 1. Bind with SO_REUSEADDR (via socket2)
        let socket = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )?;

        socket.set_reuse_address(true)?;
        #[cfg(unix)]
        socket.set_reuse_port(true)?;
        socket.set_nonblocking(true)?;
        socket.bind(&format!("0.0.0.0:{}", PORT).parse::<SocketAddr>()?.into())?;
        socket.join_multicast_v4(&MULTICAST_ADDR, &Ipv4Addr::UNSPECIFIED)?;

        let udp = UdpSocket::from_std(socket.into())?;
        let sys = Arc::new(Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            socket: Arc::new(udp),
        });

        // 2. Start Listener Loop
        let sys_clone = sys.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 1024];
            loop {
                if let Ok((len, _)) = sys_clone.socket.recv_from(&mut buf).await {
                    if let Ok(sig) = serde_json::from_slice::<Signal>(&buf[..len]) {
                        // Update Cache
                        let addr = format!("{}:{}", sig.ip, sig.port);
                        sys_clone.cache.write().await.insert(sig.cell_name, addr);
                    }
                }
            }
        });

        Ok(sys)
    }

    /// Publish my existence to the network
    pub async fn secrete(&self, cell_name: &str, port: u16) -> Result<()> {
        // Simple logic: get local IP (naive for MVP)
        let local_ip = "127.0.0.1"; // Replace with actual interface discovery in prod
        let sig = Signal {
            cell_name: cell_name.into(),
            ip: local_ip.into(),
            port,
        };
        let bytes = serde_json::to_vec(&sig)?;
        let target = format!("{}:{}", MULTICAST_ADDR, PORT);
        self.socket.send_to(&bytes, target).await?;
        Ok(())
    }

    pub async fn lookup(&self, cell_name: &str) -> Option<String> {
        self.cache.read().await.get(cell_name).cloned()
    }
}
