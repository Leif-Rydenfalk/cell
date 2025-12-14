// cells/tcp-gateway/src/main.rs
// SPDX-License-Identifier: MIT
// A generic Gateway Cell that allows Cells to connect to raw TCP endpoints.
// This proves the "Opt-in Complexity" model: features like raw TCP bridging
// are just Cells, not SDK bloat.

use cell_sdk::*;
use cell_model::bridge::{BridgeRequest, BridgeResponse};
use tokio::net::{TcpStream, UnixListener};
use tokio::io::copy;
use anyhow::{Result, anyhow};
use std::sync::Arc;

#[service]
struct TcpGateway;

#[handler]
impl TcpGateway {
    /// Handle the standard Bridge protocol.
    /// Request: Mount { target: "127.0.0.1:8080" }
    /// Response: Mounted { socket_path: "/tmp/cell/.../proxy.sock" }
    async fn mount(&self, req: BridgeRequest) -> Result<BridgeResponse> {
        let BridgeRequest::Mount { target } = req;
        
        tracing::info!("[TcpGateway] Mounting target: {}", target);

        // 1. Validate Target (simple IP:PORT check)
        if target.split(':').count() != 2 {
            return Ok(BridgeResponse::Error { 
                message: "Invalid target format. Expected IP:PORT".into() 
            });
        }

        // 2. Create a temporary Unix socket for the proxy
        let socket_dir = std::env::temp_dir().join("cell-tcp-gateway");
        tokio::fs::create_dir_all(&socket_dir).await.ok();
        
        let id: u64 = rand::random();
        let proxy_socket_path = socket_dir.join(format!("{}.sock", id));
        
        // 3. Bind Listener
        let listener = match UnixListener::bind(&proxy_socket_path) {
            Ok(l) => l,
            Err(e) => return Ok(BridgeResponse::Error { message: e.to_string() }),
        };

        // 4. Spawn the Proxy Task
        // This task lives as long as the cell does, accepting connections on the unix socket
        // and piping them to new TCP connections to the target.
        let target_clone = target.clone();
        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((mut unix_stream, _)) => {
                        let target_addr = target_clone.clone();
                        tokio::spawn(async move {
                            match TcpStream::connect(&target_addr).await {
                                Ok(mut tcp_stream) => {
                                    let (mut ri, mut wi) = unix_stream.split();
                                    let (mut ro, mut wo) = tcp_stream.split();
                                    
                                    let client_to_server = copy(&mut ri, &mut wo);
                                    let server_to_client = copy(&mut ro, &mut wi);
                                    
                                    let _ = tokio::try_join!(client_to_server, server_to_client);
                                }
                                Err(e) => {
                                    tracing::error!("Failed to connect to target {}: {}", target_addr, e);
                                }
                            }
                        });
                    }
                    Err(e) => {
                        tracing::error!("Accept error on proxy socket: {}", e);
                        break;
                    }
                }
            }
        });

        // 5. Return the path to the proxy socket
        Ok(BridgeResponse::Mounted { 
            socket_path: proxy_socket_path.to_string_lossy().to_string() 
        })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    
    tracing::info!("TcpGateway Online. Usage: Synapse::grow(\"tcp:IP:PORT\")");
    
    let service = TcpGateway;
    // Serve on the standard app channel.
    // The Synapse client knows to send BridgeRequests to channel 0 (APP) when
    // connecting to a gateway.
    service.serve("tcp-gateway").await
}