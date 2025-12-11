// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use crate::pheromones::PheromoneSystem;
use cell_model::protocol::GENOME_REQUEST;
use anyhow::{Result};
use cell_model::rkyv::ser::serializers::AllocSerializer;
use cell_model::rkyv::{self, Archive};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
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
    pub async fn ignite(cell_name: &str, node_id: u64) -> Result<Self> {
        let pheromones = PheromoneSystem::ignite(node_id).await?;
        let addrs = get_all_local_addresses().await?;

        let mut endpoints = Vec::new();

        for ip in addrs {
            match bind_quic_endpoint(ip).await {
                Ok((addr, endpoint)) => {
                    endpoints.push((addr, endpoint));
                    let port = addr.port();
                    let ip_str = addr.ip().to_string();
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
            let resp = genome.as_ref().as_ref().map(|s| s.as_bytes()).unwrap_or(&[]);
            send.write_all(&(resp.len() as u32).to_le_bytes()).await?;
            send.write_all(resp).await?;
            return Ok(());
        }

        let archived_req = rkyv::check_archived_root::<Req>(&buf)
            .map_err(|e| anyhow::anyhow!("Invalid rpc data: {:?}", e))?;

        let response = handler(archived_req).await?;
        let resp_bytes = rkyv::to_bytes::<_, 1024>(&response)?.into_vec();

        send.write_all(&(resp_bytes.len() as u32).to_le_bytes()).await?;
        send.write_all(&resp_bytes).await?;
        send.finish().await?;

        Ok(())
    }
}

pub struct AxonClient;

impl AxonClient {
    pub async fn connect(cell_name: &str) -> Result<Option<quinn::Connection>> {
        let pheromones = PheromoneSystem::ignite(0).await?;
        info!("[Axon] Discovering cell '{}'...", cell_name);
        let _ = pheromones.query(cell_name).await;
        
        let max_attempts = 30; 
        for _ in 0..max_attempts {
            let signals = pheromones.lookup_all(cell_name).await;
            if !signals.is_empty() {
                for sig in signals {
                    if let Ok(Some(conn)) = Self::connect_to_signal(&sig).await {
                         info!("[Axon] ✓ Connected to '{}' (ID: {})", cell_name, sig.instance_id);
                         return Ok(Some(conn));
                    }
                }
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
        Ok(None)
    }

    pub async fn connect_to_signal(sig: &cell_discovery::lan::Signal) -> Result<Option<quinn::Connection>> {
        let addrs = expand_signal_to_candidates(sig);
        for addr in addrs {
            if let Ok(Some(conn)) = try_connect(addr).await {
                return Ok(Some(conn));
            }
        }
        Ok(None)
    }

    pub fn make_endpoint() -> Result<quinn::Endpoint> {
        make_client_endpoint()
    }
}

async fn get_all_local_addresses() -> Result<Vec<IpAddr>> {
    let mut addrs = Vec::new();
    if let Ok(interfaces) = if_addrs::get_if_addrs() {
        for iface in interfaces {
            if iface.is_loopback() { continue; }
            addrs.push(iface.addr.ip());
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

fn expand_signal_to_candidates(sig: &cell_discovery::lan::Signal) -> Vec<SocketAddr> {
    let mut candidates = Vec::new();
    if let Ok(addr) = format!("{}:{}", sig.ip, sig.port).parse::<SocketAddr>() {
        candidates.push(addr);
    }
    candidates
}

async fn try_connect(addr: SocketAddr) -> Result<Option<quinn::Connection>> {
    let endpoint = make_client_endpoint()?;
    let timeout = tokio::time::Duration::from_millis(500);
    match endpoint.connect(addr, "localhost") {
        Ok(connecting) => match tokio::time::timeout(timeout, connecting).await {
            Ok(Ok(conn)) => Ok(Some(conn)),
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
    
    let mut transport_config = quinn::TransportConfig::default();
    transport_config.max_concurrent_uni_streams(0u8.into());
    transport_config.max_concurrent_bidi_streams(128u8.into());
    
    server_config.transport_config(Arc::new(transport_config));
    
    Ok(server_config)
}

fn make_client_endpoint() -> Result<quinn::Endpoint> {
    let mut roots = rustls::RootCertStore::empty();
    roots.add_trust_anchors(webpki_roots::TLS_SERVER_ROOTS.iter().map(|ta| {
        rustls::OwnedTrustAnchor::from_subject_spki_name_constraints(ta.subject, ta.spki, ta.name_constraints)
    }));
    
    struct DevVerifier;
    impl rustls::client::ServerCertVerifier for DevVerifier {
        fn verify_server_cert(
            &self, _end_entity: &rustls::Certificate, _intermediates: &[rustls::Certificate],
            _server_name: &rustls::ServerName, _scts: &mut dyn Iterator<Item = &[u8]>,
            _ocsp_response: &[u8], _now: std::time::SystemTime,
        ) -> Result<rustls::client::ServerCertVerified, rustls::Error> {
            Ok(rustls::client::ServerCertVerified::assertion())
        }
    }
    let crypto = rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_custom_certificate_verifier(Arc::new(DevVerifier))
        .with_no_client_auth();

    let client_config = quinn::ClientConfig::new(Arc::new(crypto));
    let mut endpoint = quinn::Endpoint::client("0.0.0.0:0".parse()?)?;
    endpoint.set_default_client_config(client_config);
    
    Ok(endpoint)
}