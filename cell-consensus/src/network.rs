use crate::LogEntry;
use anyhow::Result;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

/// Handles peer-to-peer replication traffic.
/// Listens on `port + 1` of the cell's main port.
pub struct RaftNetwork {
    peers: Vec<String>,
    inbox: broadcast::Sender<LogEntry>,
    // Store handle to abort task on Drop
    _listener_task: JoinHandle<()>,
}

impl RaftNetwork {
    pub async fn new(my_id: u64, peers: Vec<String>) -> Result<Self> {
        let (tx, _) = broadcast::channel(100);

        let port = 10000 + my_id;
        let addr = format!("0.0.0.0:{}", port);

        // Explicitly set reuseaddr/port usually not needed for basic tests if we drop correctly,
        // but helps avoiding TIME_WAIT issues in tight loops.
        let listener = TcpListener::bind(&addr).await?;
        println!("[Raft] Consensus Network Active on {}", addr);

        let tx_clone = tx.clone();

        // Spawn and keep handle
        let handle = tokio::spawn(async move {
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

        Ok(Self {
            peers,
            inbox: tx,
            _listener_task: handle,
        })
    }

    pub fn listen(&self) -> broadcast::Receiver<LogEntry> {
        self.inbox.subscribe()
    }

    pub async fn broadcast(&self, entry: LogEntry) -> Result<()> {
        let bytes = bincode::serialize(&entry)?;
        let len = (bytes.len() as u32).to_le_bytes();

        for peer in &self.peers {
            if let Ok(mut stream) = TcpStream::connect(peer).await {
                let _ = stream.write_all(&len).await;
                let _ = stream.write_all(&bytes).await;
            }
        }
        Ok(())
    }
}

impl Drop for RaftNetwork {
    fn drop(&mut self) {
        // Crucial: Kill the listener so the port is freed
        self._listener_task.abort();
    }
}
