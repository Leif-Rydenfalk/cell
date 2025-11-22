use anyhow::Result;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStderr, ChildStdout};
use tokio::sync::mpsc;
use tokio::task;

const MAX_LOG_SIZE: u64 = 10 * 1024 * 1024; // 10 MB
const BACKUP_COUNT: usize = 5;

/// The Vacuole aggregates logs from multiple sources (Colony Workers)
/// and pipes them into a centralized, thread-safe log file with rotation.
pub struct Vacuole {
    sender: mpsc::Sender<String>,
}

impl Vacuole {
    pub async fn new(log_path: PathBuf) -> Result<Self> {
        if let Some(p) = log_path.parent() {
            tokio::fs::create_dir_all(p).await?;
        }

        // Create the channel
        let (tx, mut rx) = mpsc::channel::<String>(1024);

        // Spawn the Writer Task
        task::spawn(async move {
            let file = match Self::open_log(&log_path).await {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("Vacuole failed to open log: {}", e);
                    return;
                }
            };

            // We track size manually to avoid stat() calls on every write
            let mut current_size = match file.metadata().await {
                Ok(m) => m.len(),
                Err(_) => 0,
            };

            let mut writer = tokio::io::BufWriter::new(file);

            while let Some(line) = rx.recv().await {
                let bytes = line.as_bytes();
                let len = bytes.len() as u64 + 1; // +1 for newline

                // Check Rotation
                if current_size + len > MAX_LOG_SIZE {
                    let _ = writer.flush().await;
                    drop(writer); // Release file handle

                    if let Err(e) = Self::rotate(&log_path).await {
                        eprintln!("Vacuole rotation failed: {}", e);
                    }

                    // Re-open
                    match Self::open_log(&log_path).await {
                        Ok(f) => {
                            writer = tokio::io::BufWriter::new(f);
                            current_size = 0;
                        }
                        Err(e) => {
                            eprintln!("Vacuole failed to re-open log: {}", e);
                            return;
                        }
                    }
                }

                // Write
                if writer.write_all(bytes).await.is_err() {
                    break;
                }
                if writer.write_all(b"\n").await.is_err() {
                    break;
                }

                // In a high-throughput scenario, we might rely on the BufWriter's internal buffer
                // rather than flushing every line. However, for logs, latency matters.
                // We'll flush if the channel is empty (opportunistic flush)
                if rx.is_empty() {
                    let _ = writer.flush().await;
                }

                current_size += len;
            }
        });

        Ok(Self { sender: tx })
    }

    pub fn attach(&self, id: String, stdout: Option<ChildStdout>, stderr: Option<ChildStderr>) {
        if let Some(out) = stdout {
            let tx = self.sender.clone();
            let id = id.clone();
            task::spawn(async move {
                let mut reader = BufReader::new(out).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    let _ = tx.send(format!("[{}] {}", id, line)).await;
                }
            });
        }

        if let Some(err) = stderr {
            let tx = self.sender.clone();
            let id = id.clone();
            task::spawn(async move {
                let mut reader = BufReader::new(err).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    let _ = tx.send(format!("[{}] [ERR] {}", id, line)).await;
                }
            });
        }
    }

    async fn open_log(path: &Path) -> std::io::Result<tokio::fs::File> {
        tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
    }

    async fn rotate(path: &Path) -> std::io::Result<()> {
        // log.4 -> log.5 (Delete log.5 first)
        // log.3 -> log.4
        // ...
        // log -> log.1
        for i in (0..BACKUP_COUNT).rev() {
            let src = if i == 0 {
                path.to_path_buf()
            } else {
                path.with_extension(format!("log.{}", i))
            };

            let dst = path.with_extension(format!("log.{}", i + 1));

            if src.exists() {
                let _ = tokio::fs::rename(&src, &dst).await;
            }
        }
        Ok(())
    }

    // Allow manual logging from the Supervisor
    pub async fn log(&self, id: &str, msg: &str) {
        let timestamp = humantime::format_rfc3339_seconds(std::time::SystemTime::now());
        let log_line = format!("[{}] [{}] [SUPERVISOR] {}", timestamp, id, msg);
        let _ = self.sender.send(log_line).await;
    }
}
