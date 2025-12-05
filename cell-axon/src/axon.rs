// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use crate::pheromones::PheromoneSystem;
use cell_model::protocol::GENOME_REQUEST;
use anyhow::{Result};
use cell_model::rkyv::ser::serializers::AllocSerializer;
use cell_model::rkyv::{self, Archive, Serialize};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UdpSocket;
use tracing::{info, warn};
use webpki_roots;

pub struct AxonServer {
    endpoints: Vec<(SocketAddr, quinn::Endpoint)>,
    _pheromones: Arc<PheromoneSystem>,
}

impl AxonServer {
    pub async fn ignite(cell_name: &str) -> Result<Self> {
        let pheromones = PheromoneSystem::ignite().await?;
        let addrs = get_all_local_addresses().await?;

        if addrs.is_empty() {
            warn!("[Axon] WARNING: No local addresses found, using fallback 0.0.0.0");
        }

        let mut endpoints = Vec::new();

        for ip in addrs {
            match bind_quic_endpoint(ip).await {
                Ok((addr, endpoint)) => {
                    endpoints.push((addr, endpoint));
                    let port = addr.port();
                    let ip_str = match addr {
                        SocketAddr::V4(v4) => v4.ip().to_string(),
                        SocketAddr::V6(v6) => v6.ip().to_string(),
                    };
                    let _ = pheromones.secrete_specific(cell_name, &ip_str, port).await;
                    info!("[Axon] ✓ Bound and advertising {}:{}", ip_str, port);
                }
                Err(e) => {
                    warn!("[Axon] Failed to bind {}: {}", ip, e);
                }
            }
        }

        if endpoints.is_empty() {
            anyhow::bail!("Failed to bind to any network interface");
        }

        pheromones.start_secreting(cell_name.to_string(), 0);

        Ok(Self {
            endpoints,
            _pheromones: pheromones,
        })
    }

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

        let archived_req = rkyv::check_archived_root::<Req>(&buf)
            .map_err(|e| anyhow::anyhow!("Invalid rpc data: {:?}", e))?;

        let response = handler(archived_req).await?;
        let resp_bytes = rkyv::to_bytes::<_, 1024>(&response)?.into_vec();

        send.write_all(&(resp_bytes.len() as u32).to_le_bytes())
            .await?;
        send.write_all(&resp_bytes).await?;
        send.finish().await?;

        Ok(())
    }
}

pub struct AxonClient;

impl AxonClient {
    pub async fn connect(cell_name: &str) -> Result<Option<quinn::Connection>> {
        info!("[Axon] Discovering cell '{}'...", cell_name);
        let pheromones = PheromoneSystem::ignite().await?;
        let _ = pheromones.query(cell_name).await;
        
        let max_attempts = 30; 
        for attempt in 0..max_attempts {
            let signals = pheromones.lookup_all(cell_name).await;
            if !signals.is_empty() {
                info!("[Axon] Found {} potential addresses", signals.len());
                for sig in signals {
                    let addrs = expand_signal_to_candidates(&sig);
                    for addr in addrs {
                        if let Ok(Some(conn)) = try_connect(addr).await {
                            info!("[Axon] ✓ Connected to '{}'", cell_name);
                            return Ok(Some(conn));
                        }
                    }
                }
            }
            if attempt > 0 && attempt % 10 == 0 {
                info!("[Axon] Still searching... (attempt {}/{})", attempt, max_attempts);
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
        warn!("[Axon] Could not discover '{}'", cell_name);
        Ok(None)
    }

    pub async fn connect_exact(addr: &str) -> Result<Option<quinn::Connection>> {
        let socket_addr: SocketAddr = addr.parse()?;
        try_connect(socket_addr).await
    }

    pub async fn fire<Req>(conn: &quinn::Connection, request: &Req) -> Result<Vec<u8>>
    where
        Req: Serialize<AllocSerializer<1024>>,
    {
        const RPC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

        let fut = async {
             let req_bytes = rkyv::to_bytes::<_, 1024>(request)?.into_vec();
             let (mut send, mut recv) = conn.open_bi().await?;

             send.write_all(&(req_bytes.len() as u32).to_le_bytes()).await?;
             send.write_all(&req_bytes).await?;
             send.finish().await?;

             let mut len_buf = [0u8; 4];
             recv.read_exact(&mut len_buf).await?;
             let len = u32::from_le_bytes(len_buf) as usize;

             let mut resp_bytes = vec![0u8; len];
             recv.read_exact(&mut resp_bytes).await?;

             Ok(resp_bytes)
        };

        match tokio::time::timeout(RPC_TIMEOUT, fut).await {
            Ok(res) => res,
            Err(_) => anyhow::bail!("RPC timeout after {:?}", RPC_TIMEOUT),
        }
    }

    pub fn make_endpoint() -> Result<quinn::Endpoint> {
        make_client_endpoint()
    }
}

// ---------- Internal Helpers ----------

async fn get_all_local_addresses() -> Result<Vec<IpAddr>> {
    let mut addrs = Vec::new();
    if let Ok(interfaces) = if_addrs::get_if_addrs() {
        for iface in interfaces {
            if iface.is_loopback() { continue; }
            let ip = iface.addr.ip();
            match ip {
                IpAddr::V4(v4) => {
                    if v4.octets()[0] == 169 && v4.octets()[1] == 254 { continue; }
                }
                IpAddr::V6(v6) => {
                    if v6.segments()[0] == 0xfe80 { continue; }
                }
            }
            addrs.push(ip);
        }
    }
    if addrs.is_empty() {
        if let Ok(ip) = local_ip_address::local_ip() { addrs.push(ip); }
    }
    if addrs.is_empty() {
        addrs.push(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)));
    }
    Ok(addrs)
}

async fn bind_quic_endpoint(ip: IpAddr) -> Result<(SocketAddr, quinn::Endpoint)> {
    let sock = UdpSocket::bind(SocketAddr::new(ip, 0)).await?;
    let local_addr = sock.local_addr()?;
    let endpoint = quinn::Endpoint::new(
        quinn::EndpointConfig::default(),
        Some(make_server_config()?),
        sock.into_std()?,
        Arc::new(quinn::TokioRuntime),
    )?;
    Ok((local_addr, endpoint))
}

fn expand_signal_to_candidates(sig: &crate::pheromones::Signal) -> Vec<SocketAddr> {
    let mut candidates = Vec::new();
    if let Ok(addr) = format!("{}:{}", sig.ip, sig.port).parse::<SocketAddr>() {
        candidates.push(addr);
    }
    if let Ok(ip) = sig.ip.parse::<Ipv4Addr>() {
        candidates.push(SocketAddr::new(IpAddr::V4(ip), sig.port));
        let octets = ip.octets();
        let broadcast = Ipv4Addr::new(octets[0], octets[1], octets[2], 255);
        candidates.push(SocketAddr::new(IpAddr::V4(broadcast), sig.port));
    } else if let Ok(ip) = sig.ip.parse::<Ipv6Addr>() {
        candidates.push(SocketAddr::new(IpAddr::V6(ip), sig.port));
    }
    candidates
}

async fn try_connect(addr: SocketAddr) -> Result<Option<quinn::Connection>> {
    let endpoint = make_client_endpoint()?;
    let timeout = tokio::time::Duration::from_millis(500);
    match endpoint.connect(addr, "localhost") {
        Ok(connecting) => match tokio::time::timeout(timeout, connecting).await {
            Ok(Ok(conn)) => {
                info!("[Axon] ✓ Connected to {}", addr);
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
    let mut roots = rustls::RootCertStore::empty();
    
    roots.add_trust_anchors(webpki_roots::TLS_SERVER_ROOTS.iter().map(|ta| {
        rustls::OwnedTrustAnchor::from_subject_spki_name_constraints(
            ta.subject,
            ta.spki,
            ta.name_constraints,
        )
    }));

    if let Ok(cert_path) = std::env::var("CELL_TRUSTED_CERT") {
        if let Ok(cert_data) = std::fs::read(&cert_path) {
             let mut reader = std::io::BufReader::new(&cert_data[..]);
             if let Ok(certs) = rustls_pemfile::certs(&mut reader) {
                 for cert in certs {
                    let _ = roots.add(&rustls::Certificate(cert));
                 }
             }
        }
    }
    
    let crypto = if std::env::var("CELL_DEV_MODE").is_ok() {
        warn!("WARNING: Running in DEV_MODE with relaxed TLS verification");
        
        struct DevVerifier;
        impl rustls::client::ServerCertVerifier for DevVerifier {
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
        
        rustls::ClientConfig::builder()
            .with_safe_defaults()
            .with_custom_certificate_verifier(Arc::new(DevVerifier))
            .with_no_client_auth()
    } else {
        rustls::ClientConfig::builder()
            .with_safe_defaults()
            .with_root_certificates(roots)
            .with_no_client_auth()
    };

    let client_config = quinn::ClientConfig::new(Arc::new(crypto));
    let mut endpoint = quinn::Endpoint::client("0.0.0.0:0".parse()?)?;
    endpoint.set_default_client_config(client_config);

    Ok(endpoint)
}