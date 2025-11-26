//! # Pheromones (Endocrine System)
//!
//! This module handles the UDP Multicast discovery mechanism for the Cell network.
//! Cells broadcast "Pheromones" (Heartbeats) to announce their presence, capabilities,
//! and location to the local network segment.
//!
//! ## Biological Metaphor
//! Just as cells release chemicals to signal neighbors, this system releases UDP packets.
//! It allows for "Organic Discovery"—no central registry is required for local peers to find each other.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use socket2::{Domain, Protocol, Socket, Type};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::time::Duration;
use tokio::sync::mpsc;

/// The standard multicast group for Cell discovery.
const MULTICAST_ADDR: Ipv4Addr = Ipv4Addr::new(239, 255, 0, 1);
/// The standard port for Pheromone traffic.
const PORT: u16 = 9099;

/// A chemical signal broadcast to the network.
/// Contains all necessary information to establish a connection (Synapse).
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Pheromone {
    /// The unique name of the specific cell instance (e.g., "worker-01").
    pub cell_name: String,
    /// The semantic group this cell belongs to (e.g., "worker").
    /// Used for load balancing.
    pub service_group: String,
    /// The TCP address (Axon) where this cell accepts encrypted connections.
    pub tcp_addr: SocketAddr,
    /// The public identity key (Antigen) of this cell.
    pub public_key: String,
    /// If true, this cell is willing to accept paid work (ATP) from strangers.
    pub is_donor: bool,
    /// The absolute path to the local Unix socket (Gap Junction).
    ///
    /// ## Optimization
    /// If a peer receives this and sees that the file exists on its own disk,
    /// it will upgrade the connection to a raw Unix socket, bypassing TCP/Encryption
    /// for maximum throughput (~5GB/s).
    pub ipc_socket: Option<String>,
}

/// The manager for sending and receiving Pheromones.
pub struct EndocrineSystem;

impl EndocrineSystem {
    /// Starts the background discovery tasks.
    ///
    /// # Arguments
    /// * `my_name` - Unique ID of this cell.
    /// * `service_group` - The service this cell provides.
    /// * `my_tcp_port` - The port the Golgi Axon is listening on.
    /// * `my_pub_key` - The Curve25519 public key string.
    /// * `is_donor` - Whether to advertise donor status.
    /// * `my_ipc_socket` - The local file path to the listening Unix socket (for local optimization).
    ///
    /// # Returns
    /// A channel receiver that streams discovered peers.
    pub async fn start(
        my_name: String,
        service_group: String,
        my_tcp_port: u16,
        my_pub_key: String,
        is_donor: bool,
        my_ipc_socket: Option<String>,
    ) -> Result<mpsc::Receiver<Pheromone>> {
        let (tx, rx) = mpsc::channel(32);

        // 1. Setup Sender (Broadcaster)
        // We bind to ephemeral port 0 because we only send *to* the multicast group.
        let send_socket = UdpSocket::bind("0.0.0.0:0")?;
        send_socket.set_broadcast(true)?;

        // 2. Setup Receiver (Listener)
        // We must use socket2 to set SO_REUSEPORT/SO_REUSEADDR so multiple cells
        // on the same machine can listen to the same multicast group without conflict.
        let recv_socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
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

        // Convert to Tokio socket for async usage
        let recv_socket = tokio::net::UdpSocket::from_std(recv_socket.into())?;

        // 3. Construct Identity Packet
        // Determine local IP for the TCP address (best effort)
        let my_ip = get_local_ip().unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));

        let my_info = Pheromone {
            cell_name: my_name,
            service_group,
            tcp_addr: SocketAddr::new(my_ip, my_tcp_port),
            public_key: my_pub_key,
            is_donor,
            ipc_socket: my_ipc_socket, // Advertise local path for optimization
        };

        // 4. Spawn Heartbeat Task (The Broadcaster)
        if my_tcp_port > 0 {
            let sender_info = my_info.clone();
            tokio::spawn(async move {
                let msg = serde_json::to_vec(&sender_info).unwrap();
                let target = format!("{}:{}", MULTICAST_ADDR, PORT);
                loop {
                    let _ = send_socket.send_to(&msg, &target);

                    // Add jitter to prevent packet synchronization/collisions on large LANs
                    let jitter = rand::random::<u64>() % 1000;
                    tokio::time::sleep(Duration::from_millis(3000 + jitter)).await;
                }
            });
        }

        // 5. Spawn Listener Task (The Receptor)
        let receiver_info = my_info.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 4096]; // Buffer large enough for Pheromone struct + Paths
            loop {
                if let Ok((len, _addr)) = recv_socket.recv_from(&mut buf).await {
                    if let Ok(p) = serde_json::from_slice::<Pheromone>(&buf[..len]) {
                        // Filter out echoes of our own voice
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

/// Helper to determine the machine's primary local IP address.
/// Connects to a public DNS (8.8.8.8) to see which interface the OS selects.
/// Does not actually send packets.
fn get_local_ip() -> Option<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    socket.local_addr().ok().map(|addr| addr.ip())
}
