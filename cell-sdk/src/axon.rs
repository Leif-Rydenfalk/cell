// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

//! # Axon - Nuclear Discovery Mode (Always Active)
//!
//! This module handles long-distance neural pathways with aggressive multi-interface discovery.

#![cfg(feature = "axon")]

use crate::pheromones::PheromoneSystem;
use crate::protocol::GENOME_REQUEST;
use crate::synapse::Response;
use anyhow::{Context, Result};
use rkyv::ser::serializers::AllocSerializer;
use rkyv::{Archive, Serialize};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UdpSocket;

// --- Axon Server (Multi-Interface Listener) ---

pub struct AxonServer {
    endpoints: Vec<(SocketAddr, quinn::Endpoint)>,
    _pheromones: Arc<PheromoneSystem>,
}

impl AxonServer {
    /// Ignites the LAN interface: Binds to EVERY local address automatically
    pub async fn ignite(cell_name: &str) -> Result<Self> {
        // 1. Start pheromone system
        let pheromones = PheromoneSystem::ignite().await?;

        // 2. Get all local addresses
        let addrs = get_all_local_addresses().await?;

        if addrs.is_empty() {
            eprintln!("[Axon] WARNING: No local addresses found, using fallback 0.0.0.0");
        }

        let mut endpoints = Vec::new();

        // 3. Bind QUIC endpoint to each address
        for ip in addrs {
            match bind_quic_endpoint(ip).await {
                Ok((addr, endpoint)) => {
                    endpoints.push((addr, endpoint));

                    // 4. Advertise this specific address
                    let port = addr.port();
                    let ip_str = match addr {
                        SocketAddr::V4(v4) => v4.ip().to_string(),
                        SocketAddr::V6(v6) => v6.ip().to_string(),
                    };

                    // Create signal with this specific IP
                    pheromones
                        .secrete_specific(cell_name, &ip_str, port)
                        .await?;

                    println!("[Axon] ✓ Bound and advertising {}:{}", ip_str, port);
                }
                Err(e) => {
                    eprintln!("[Axon] Failed to bind {}: {}", ip, e);
                }
            }
        }

        if endpoints.is_empty() {
            anyhow::bail!("Failed to bind to any network interface");
        }

        // Start continuous advertising
        pheromones.start_secreting(cell_name.to_string(), 0);

        Ok(Self {
            endpoints,
            _pheromones: pheromones,
        })
    }

    /// Accepts connections from ANY bound endpoint (multiplexed)
    pub async fn accept(&self) -> Option<quinn::Connecting> {
        use futures::stream::{FuturesUnordered, StreamExt};

        let mut acceptors: FuturesUnordered<_> = self
            .endpoints
            .iter()
            .map(|(_, ep)| Box::pin(ep.accept()))
            .collect();

        loop {
            match acceptors.next().await? {
                Some(connecting) => return Some(connecting),
                None => {
                    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                }
            }
        }
    }

    pub async fn handle_rpc_stream<F, Req, Resp>(
        mut send: quinn::SendStream,
        mut recv: quinn::RecvStream,
        handler: F,
        genome: Arc<Option<String>>,
    ) -> Result<()>
    where
        F: for<'a> Fn(
            &'a Req::Archived,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<Resp>> + Send + 'a>,
        >,
        Req: Archive,
        Req::Archived: for<'a> rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'a>>,
        Resp: rkyv::Serialize<AllocSerializer<1024>>,
    {
        let mut len_buf = [0u8; 4];
        recv.read_exact(&mut len_buf).await?;
        let len = u32::from_le_bytes(len_buf) as usize;

        let mut buf = vec![0u8; len];
        recv.read_exact(&mut buf).await?;

        // Handle genome request
        if buf == GENOME_REQUEST {
            let resp = genome
                .as_ref()
                .as_ref()
                .map(|s| s.as_bytes())
                .unwrap_or(&[]);
            send.write_all(&(resp.len() as u32).to_le_bytes()).await?;
            send.write_all(resp).await?;
            return Ok(());
        }

        // Handle RPC
        let archived_req = rkyv::check_archived_root::<Req>(&buf)
            .map_err(|e| anyhow::anyhow!("Invalid request format: {:?}", e))?;

        let response = handler(archived_req).await?;
        let resp_bytes = rkyv::to_bytes::<_, 1024>(&response)?.into_vec();

        send.write_all(&(resp_bytes.len() as u32).to_le_bytes())
            .await?;
        send.write_all(&resp_bytes).await?;
        send.finish().await?;

        Ok(())
    }
}

// --- Axon Client (Aggressive Multi-Strategy Discovery) ---

pub struct AxonClient;

impl AxonClient {
    /// Nuclear discovery: Tries EVERY strategy concurrently until success
    pub async fn connect(cell_name: &str) -> Result<Option<quinn::Connection>> {
        println!("[Axon] Discovering cell '{}'...", cell_name);

        // 1. Start pheromone system
        let pheromones = PheromoneSystem::ignite().await?;

        // 2. Send active discovery query
        let _ = pheromones.query(cell_name).await;

        // 3. Aggressive polling with expanding timeout
        let max_attempts = 30; // 30 attempts = ~3 seconds with 100ms sleep

        for attempt in 0..max_attempts {
            // Check all known signals
            let signals = pheromones.lookup_all(cell_name).await;

            if !signals.is_empty() {
                println!(
                    "[Axon] Found {} potential addresses for '{}'",
                    signals.len(),
                    cell_name
                );

                // Try all addresses concurrently
                let mut tasks = Vec::new();
                for sig in signals {
                    let addrs = expand_signal_to_candidates(&sig);
                    for addr in addrs {
                        tasks.push(tokio::spawn(async move { try_connect(addr).await }));
                    }
                }

                // Wait for first success
                for task in tasks {
                    if let Ok(Ok(Some(conn))) = task.await {
                        println!("[Axon] ✓ Connected to '{}'", cell_name);
                        return Ok(Some(conn));
                    }
                }
            }

            // Show progress
            if attempt % 10 == 0 && attempt > 0 {
                println!(
                    "[Axon] Still searching... (attempt {}/{})",
                    attempt, max_attempts
                );
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }

        println!("[Axon] Could not discover '{}'", cell_name);
        Ok(None)
    }

    /// Connect to exact address (used by discovery probing)
    pub async fn connect_exact(addr: &str) -> Result<Option<quinn::Connection>> {
        let socket_addr: SocketAddr = addr.parse()?;
        try_connect(socket_addr).await
    }

    /// Send RPC over existing connection
    pub async fn fire<Req, Resp>(conn: &quinn::Connection, request: &Req) -> Result<Response<Resp>>
    where
        Req: Serialize<AllocSerializer<1024>>,
        Resp: Archive,
        Resp::Archived: 'static,
    {
        let req_bytes = crate::rkyv::to_bytes::<_, 1024>(request)?.into_vec();

        let (mut send, mut recv) = conn.open_bi().await?;

        send.write_all(&(req_bytes.len() as u32).to_le_bytes())
            .await?;
        send.write_all(&req_bytes).await?;
        send.finish().await?;

        let mut len_buf = [0u8; 4];
        recv.read_exact(&mut len_buf).await?;
        let len = u32::from_le_bytes(len_buf) as usize;

        let mut resp_bytes = vec![0u8; len];
        recv.read_exact(&mut resp_bytes).await?;

        Ok(Response::Owned(resp_bytes))
    }

    pub fn make_endpoint() -> Result<quinn::Endpoint> {
        make_client_endpoint()
    }
}

// ---------- Internal Helpers ----------

/// Get ALL local IP addresses (IPv4 and IPv6)
async fn get_all_local_addresses() -> Result<Vec<IpAddr>> {
    let mut addrs = Vec::new();

    // Get from interfaces
    if let Ok(interfaces) = if_addrs::get_if_addrs() {
        for iface in interfaces {
            // Skip loopback unless explicitly enabled
            if iface.is_loopback() {
                continue;
            }

            let ip = iface.addr.ip();

            // Filter out link-local IPv6 (fe80::) and IPv4 (169.254.x.x)
            match ip {
                IpAddr::V4(v4) => {
                    if v4.octets()[0] == 169 && v4.octets()[1] == 254 {
                        continue;
                    }
                }
                IpAddr::V6(v6) => {
                    if v6.segments()[0] == 0xfe80 {
                        continue;
                    }
                }
            }

            addrs.push(ip);
        }
    }

    // Fallback: try to get primary local IP
    if addrs.is_empty() {
        if let Ok(ip) = local_ip_address::local_ip() {
            addrs.push(ip);
        }
    }

    // Last resort fallback
    if addrs.is_empty() {
        addrs.push(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)));
    }

    Ok(addrs)
}

/// Bind a QUIC endpoint to specific IP
async fn bind_quic_endpoint(ip: IpAddr) -> Result<(SocketAddr, quinn::Endpoint)> {
    // Create UDP socket
    let sock = UdpSocket::bind(SocketAddr::new(ip, 0)).await?;
    let local_addr = sock.local_addr()?;

    // Create QUIC endpoint
    let endpoint = quinn::Endpoint::new(
        quinn::EndpointConfig::default(),
        Some(make_server_config()?),
        sock.into_std()?,
        Arc::new(quinn::TokioRuntime),
    )?;

    Ok((local_addr, endpoint))
}

/// Expand a signal into multiple candidate addresses to try
fn expand_signal_to_candidates(sig: &crate::pheromones::Signal) -> Vec<SocketAddr> {
    let mut candidates = Vec::new();

    // 1. Exact advertised address
    if let Ok(addr) = format!("{}:{}", sig.ip, sig.port).parse::<SocketAddr>() {
        candidates.push(addr);
    }

    // 2. Parse as IP and try different approaches
    if let Ok(ip) = sig.ip.parse::<Ipv4Addr>() {
        candidates.push(SocketAddr::new(IpAddr::V4(ip), sig.port));

        // Try broadcast address
        let octets = ip.octets();
        let broadcast = Ipv4Addr::new(octets[0], octets[1], octets[2], 255);
        candidates.push(SocketAddr::new(IpAddr::V4(broadcast), sig.port));
    } else if let Ok(ip) = sig.ip.parse::<Ipv6Addr>() {
        candidates.push(SocketAddr::new(IpAddr::V6(ip), sig.port));
    }

    candidates
}

/// Try to connect to a specific address
async fn try_connect(addr: SocketAddr) -> Result<Option<quinn::Connection>> {
    let endpoint = make_client_endpoint()?;

    // Short timeout for each attempt
    let timeout = tokio::time::Duration::from_millis(500);

    match endpoint.connect(addr, "localhost") {
        Ok(connecting) => match tokio::time::timeout(timeout, connecting).await {
            Ok(Ok(conn)) => {
                println!("[Axon] ✓ Connected to {}", addr);
                Ok(Some(conn))
            }
            _ => Ok(None),
        },
        Err(_) => Ok(None),
    }
}

fn make_server_config() -> Result<quinn::ServerConfig> {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()])?;
    let cert_der = cert.serialize_der()?;
    let key_der = cert.serialize_private_key_der();
    let priv_key = rustls::PrivateKey(key_der);
    let cert_chain = vec![rustls::Certificate(cert_der)];

    let mut server_config = quinn::ServerConfig::with_single_cert(cert_chain, priv_key)?;
    let transport_config = Arc::get_mut(&mut server_config.transport).unwrap();
    transport_config.max_concurrent_uni_streams(0_u8.into());

    Ok(server_config)
}

fn make_client_endpoint() -> Result<quinn::Endpoint> {
    struct SkipVerify;
    impl rustls::client::ServerCertVerifier for SkipVerify {
        fn verify_server_cert(
            &self,
            _end_entity: &rustls::Certificate,
            _intermediates: &[rustls::Certificate],
            _server_name: &rustls::ServerName,
            _scts: &mut dyn Iterator<Item = &[u8]>,
            _ocsp_response: &[u8],
            _now: std::time::SystemTime,
        ) -> Result<rustls::client::ServerCertVerified, rustls::Error> {
            Ok(rustls::client::ServerCertVerified::assertion())
        }
    }

    let crypto = rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_custom_certificate_verifier(Arc::new(SkipVerify))
        .with_no_client_auth();

    let client_config = quinn::ClientConfig::new(Arc::new(crypto));
    let mut endpoint = quinn::Endpoint::client("0.0.0.0:0".parse()?)?;
    endpoint.set_default_client_config(client_config);

    Ok(endpoint)
}
