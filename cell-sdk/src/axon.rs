// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

//! # Axon
//!
//! The Axon module handles long-distance neural pathways (Network Transport).
//! It encapsulates QUIC (quinn), TLS (rustls), and Discovery (Pheromones).

#![cfg(feature = "axon")]

use crate::pheromones::PheromoneSystem;
use crate::protocol::GENOME_REQUEST;
use crate::synapse::Response;
use anyhow::{Context, Result};
use rkyv::ser::serializers::AllocSerializer;
use rkyv::{Archive, Serialize};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// --- Axon Server (The Listener) ---

pub struct AxonServer {
    endpoint: quinn::Endpoint,
    // Kept alive to maintain background broadcasting
    _pheromones: Arc<PheromoneSystem>,
}

impl AxonServer {
    /// Ignites the LAN interface: Binds UDP, generates Certs, starts Broadcasting.
    pub async fn ignite(cell_name: &str) -> Result<Self> {
        // 1. Ignite Pheromones
        let pheromones = PheromoneSystem::ignite().await?;

        // 2. Generate Self-Signed Certs (Ephemeral Identity)
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()])?;
        let cert_der = cert.serialize_der()?;
        let priv_key = cert.serialize_private_key_der();

        let priv_key = rustls::PrivateKey(priv_key);
        let cert_chain = vec![rustls::Certificate(cert_der)];

        let mut server_config = quinn::ServerConfig::with_single_cert(cert_chain, priv_key)?;
        // Configure transport
        let transport_config = Arc::get_mut(&mut server_config.transport).unwrap();
        transport_config.max_concurrent_uni_streams(0_u8.into()); // We only use Bi-streams

        // 3. Bind to Random Port
        let endpoint = quinn::Endpoint::server(server_config, "[::]:0".parse()?)?;
        let port = endpoint.local_addr()?.port();

        // 4. Secrete Pheromones (Announce Presence)
        println!(
            "[{}] 🌐 Axon Active. Listening on {}:{}",
            cell_name,
            PheromoneSystem::local_ip(),
            port
        );
        pheromones.start_secreting(cell_name.to_string(), port);

        Ok(Self {
            endpoint,
            _pheromones: pheromones,
        })
    }

    /// Accepts the next incoming connection
    pub async fn accept(&self) -> Option<quinn::Connecting> {
        self.endpoint.accept().await
    }

    /// Handles a specific RPC stream on the server side
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
        // 1. Read Length Header
        let mut len_buf = [0u8; 4];
        recv.read_exact(&mut len_buf).await?;
        let len = u32::from_le_bytes(len_buf) as usize;

        // 2. Read Payload
        let mut buf = vec![0u8; len];
        recv.read_exact(&mut buf).await?;

        // 3. Protocol: Genome Request
        if buf == GENOME_REQUEST {
            let resp = if let Some(json) = genome.as_ref() {
                json.as_bytes()
            } else {
                &[]
            };
            send.write_all(&(resp.len() as u32).to_le_bytes()).await?;
            send.write_all(resp).await?;
            return Ok(());
        }

        // 4. Protocol: RPC
        let archived_req = rkyv::check_archived_root::<Req>(&buf)
            .map_err(|e| anyhow::anyhow!("Invalid request format: {:?}", e))?;

        let response = handler(archived_req).await?;

        // 5. Serialize Response
        let resp_bytes = rkyv::to_bytes::<_, 1024>(&response)?.into_vec();
        send.write_all(&(resp_bytes.len() as u32).to_le_bytes())
            .await?;
        send.write_all(&resp_bytes).await?;

        // 6. Finish (EOF)
        send.finish().await?;

        Ok(())
    }
}

// --- Axon Client (The Connector) ---

pub struct AxonClient;

impl AxonClient {
    /// Attempts to find and connect to a cell via LAN
    pub async fn connect(cell_name: &str) -> Result<Option<quinn::Connection>> {
        // 1. Ignite Discovery
        let pheromones = PheromoneSystem::ignite().await?;

        // Short wait for multicast response
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        // 2. Lookup
        if let Some(signal) = pheromones.lookup(cell_name).await {
            let addr_str = format!("{}:{}", signal.ip, signal.port);
            println!("[Axon] 🔭 Discovered {} at {} via LAN", cell_name, addr_str);

            // 3. Configure TLS (Trust-on-first-use / Skip Verify for Demo)
            let endpoint = Self::make_endpoint()?;
            let addr = addr_str.parse().context("Invalid IP from discovery")?;

            // 4. Connect
            let connection = endpoint.connect(addr, "localhost")?.await?;
            Ok(Some(connection))
        } else {
            Ok(None)
        }
    }

    /// Connect to an exact address (used by auto-discovery)
    pub async fn connect_exact(addr: &str) -> Result<Option<quinn::Connection>> {
        let endpoint = Self::make_endpoint()?;
        let addr = addr.parse()?;
        match endpoint.connect(addr, "localhost")?.await {
            Ok(conn) => Ok(Some(conn)),
            Err(_) => Ok(None),
        }
    }

    /// Sends an RPC request over an existing QUIC connection
    pub async fn fire<Req, Resp>(conn: &quinn::Connection, request: &Req) -> Result<Response<Resp>>
    where
        Req: Serialize<AllocSerializer<1024>>,
        Resp: Archive,
        Resp::Archived: 'static,
    {
        let req_bytes = crate::rkyv::to_bytes::<_, 1024>(request)?.into_vec();

        // New Bidirectional Stream per Request
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
        struct SkipServerVerification;
        impl rustls::client::ServerCertVerifier for SkipServerVerification {
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
            .with_custom_certificate_verifier(Arc::new(SkipServerVerification))
            .with_no_client_auth();

        let client_config = quinn::ClientConfig::new(Arc::new(crypto));
        let mut endpoint = quinn::Endpoint::client("[::]:0".parse()?)?;
        endpoint.set_default_client_config(client_config);
        Ok(endpoint)
    }
}
