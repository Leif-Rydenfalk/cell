use anyhow::Result;
use serde::{Deserialize, Serialize};
use socket2::{Domain, Protocol, Socket, Type};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::time::Duration;
use tokio::sync::mpsc;

// Multicast Group: All Cells listen here
const MULTICAST_ADDR: Ipv4Addr = Ipv4Addr::new(239, 255, 0, 1);
const PORT: u16 = 9099;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Pheromone {
    pub cell_name: String,
    pub tcp_addr: SocketAddr,
    pub public_key: String,
}

pub struct EndocrineSystem;

impl EndocrineSystem {
    /// Starts the UDP Multicast loop.
    /// Returns a channel where discovered peers will be sent.
    pub async fn start(
        my_name: String,
        my_tcp_port: u16,
        my_pub_key: String,
    ) -> Result<mpsc::Receiver<Pheromone>> {
        let (tx, rx) = mpsc::channel(32);

        // 1. Setup Send Socket (Broadcast capable)
        // We use this to shout "I am here!"
        let send_socket = UdpSocket::bind("0.0.0.0:0")?;
        send_socket.set_broadcast(true)?;

        // 2. Setup Receive Socket (Multicast)
        // We use this to hear others.
        let recv_socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;

        // Allow multiple cells on the same machine to listen to 9099
        recv_socket.set_reuse_address(true)?;
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let fd = recv_socket.as_raw_fd();
            unsafe {
                let opt = 1;
                libc::setsockopt(
                    fd,
                    libc::SOL_SOCKET,
                    libc::SO_REUSEPORT,
                    &opt as *const _ as *const libc::c_void,
                    4,
                );
            }
        }

        recv_socket.bind(&format!("0.0.0.0:{}", PORT).parse::<SocketAddr>()?.into())?;
        recv_socket.join_multicast_v4(&MULTICAST_ADDR, &Ipv4Addr::UNSPECIFIED)?;
        recv_socket.set_nonblocking(true)?;

        let recv_socket = tokio::net::UdpSocket::from_std(recv_socket.into())?;

        // 3. Determine my local IP (Best guess)
        let my_ip = get_local_ip().unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
        let my_info = Pheromone {
            cell_name: my_name,
            tcp_addr: SocketAddr::new(my_ip, my_tcp_port),
            public_key: my_pub_key,
        };

        // 4. Spawn Pulse (Sender) - ONLY IF WE HAVE A PORT
        if my_tcp_port > 0 {
            let sender_info = my_info.clone();
            tokio::spawn(async move {
                let msg = serde_json::to_vec(&sender_info).unwrap();
                let target = format!("{}:{}", MULTICAST_ADDR, PORT);
                loop {
                    // Send Pheromone
                    let _ = send_socket.send_to(&msg, &target);

                    // Pulse every 3 seconds + jitter
                    let jitter = rand::random::<u64>() % 1000;
                    tokio::time::sleep(Duration::from_millis(3000 + jitter)).await;
                }
            });
        }

        // 5. Spawn Receptor (Listener) - ALWAYS
        let receiver_info = my_info.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 1024];
            loop {
                if let Ok((len, _addr)) = recv_socket.recv_from(&mut buf).await {
                    if let Ok(p) = serde_json::from_slice::<Pheromone>(&buf[..len]) {
                        // Don't discover self
                        if p.public_key != receiver_info.public_key {
                            let _ = tx.send(p).await;
                        }
                    }
                }
            }
        });

        Ok(rx)
    }
}

fn get_local_ip() -> Option<IpAddr> {
    // Quick hack to find local non-loopback IP
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    socket.local_addr().ok().map(|addr| addr.ip())
}
