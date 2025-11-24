use crate::LogEntry;
use anyhow::Result;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;

/// Handles peer-to-peer replication traffic.
/// Listens on `port + 1` of the cell's main port.
pub struct RaftNetwork {
    peers: Vec<String>,
    inbox: broadcast::Sender<LogEntry>,
}

impl RaftNetwork {
    pub async fn new(my_id: u64, peers: Vec<String>) -> Result<Self> {
        let (tx, _) = broadcast::channel(100);
        
        // Hack: We assume for this demo that the consensus port is 10000 + id
        let port = 10000 + my_id;
        let addr = format!("0.0.0.0:{}", port);
        
        let listener = TcpListener::bind(&addr).await?;
        println!("[Raft] Consensus Network Active on {}", addr);

        let tx_clone = tx.clone();
        tokio::spawn(async move {
            loop {
                if let Ok((mut socket, _)) = listener.accept().await {
                    let tx = tx_clone.clone();
                    tokio::spawn(async move {
                        let mut len_buf = [0u8; 4];
                        if socket.read_exact(&mut len_buf).await.is_ok() {
                            let len = u32::from_le_bytes(len_buf) as usize;
                            let mut buf = vec![0u8; len];
                            if socket.read_exact(&mut buf).await.is_ok() {
                                if let Ok(entry) = bincode::deserialize(&buf) {
                                    let _ = tx.send(entry);
                                }
                            }
                        }
                    });
                }
            }
        });

        Ok(Self { peers, inbox: tx })
    }

    pub fn listen(&self) -> broadcast::Receiver<LogEntry> {
        self.inbox.subscribe()
    }

    pub async fn broadcast(&self, entry: LogEntry) -> Result<()> {
        let bytes = bincode::serialize(&entry)?;
        let len = (bytes.len() as u32).to_le_bytes();

        for peer in &self.peers {
            // In a real system, we keep persistent connections.
            // Here we open-send-close for simplicity.
            if let Ok(mut stream) = TcpStream::connect(peer).await {
                let _ = stream.write_all(&len).await;
                let _ = stream.write_all(&bytes).await;
            }
        }
        Ok(())
    }
}