use anyhow::Result;
use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::thread;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{ChildStderr, ChildStdout};
use tokio::sync::mpsc; // Use Tokio's MPSC for async backpressure
use tokio::task;

const MAX_LOG_SIZE: u64 = 10 * 1024 * 1024; // 10 MB
const BACKUP_COUNT: usize = 5;
const CHANNEL_CAPACITY: usize = 4096; // Restore Backpressure

pub struct Vacuole {
    sender: mpsc::Sender<String>,
    _handle: Option<thread::JoinHandle<()>>,
}

impl Vacuole {
    pub async fn new(log_path: PathBuf) -> Result<Self> {
        if let Some(p) = log_path.parent() {
            tokio::fs::create_dir_all(p).await?;
        }

        // Bounded Channel.
        // If this fills up, async senders will 'await', slowing down the app
        // instead of eating all your RAM.
        let (tx, mut rx) = mpsc::channel::<String>(CHANNEL_CAPACITY);
        let path_clone = log_path.clone();

        let handle = thread::spawn(move || {
            let file = match OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path_clone)
            {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("[VACUOLE] Failed to open log: {}", e);
                    return;
                }
            };

            let mut current_size = file.metadata().map(|m| m.len()).unwrap_or(0);
            let mut writer = BufWriter::new(file);

            // USE BLOCKING RECEIVE
            // This allows the OS thread to consume messages from the Tokio channel
            while let Some(line) = rx.blocking_recv() {
                let bytes = line.as_bytes();
                let len = bytes.len() as u64 + 1;

                if current_size + len > MAX_LOG_SIZE {
                    let _ = writer.flush();
                    drop(writer);

                    if let Err(e) = Self::rotate(&path_clone) {
                        eprintln!("[VACUOLE] Rotation failed: {}", e);
                    }

                    match OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&path_clone)
                    {
                        Ok(f) => {
                            current_size = 0;
                            writer = BufWriter::new(f);
                        }
                        Err(e) => {
                            eprintln!("[VACUOLE] Failed to re-open log: {}", e);
                            return;
                        }
                    }
                }

                let _ = writer.write_all(bytes);
                let _ = writer.write_all(b"\n");

                // RESTORED: Smart Flushing
                // We verify if there are more messages waiting.
                // If the channel is empty, we flush immediately (low latency).
                // If the channel has data, we keep writing (high throughput).
                if rx.is_empty() {
                    let _ = writer.flush();
                }

                current_size += len;
            }
        });

        Ok(Self {
            sender: tx,
            _handle: Some(handle),
        })
    }

    fn rotate(path: &Path) -> std::io::Result<()> {
        for i in (0..BACKUP_COUNT).rev() {
            let src = if i == 0 {
                path.to_path_buf()
            } else {
                path.with_extension(format!("log.{}", i))
            };
            let dst = path.with_extension(format!("log.{}", i + 1));
            if src.exists() {
                if dst.exists() {
                    let _ = fs::remove_file(&dst);
                }
                let _ = fs::rename(&src, &dst);
            }
        }
        Ok(())
    }

    pub fn attach(&self, id: String, stdout: Option<ChildStdout>, stderr: Option<ChildStderr>) {
        if let Some(out) = stdout {
            let tx = self.sender.clone();
            let id = id.clone();
            task::spawn(async move {
                let mut reader = BufReader::new(out).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    // Async backpressure applies here
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

    pub async fn log(&self, id: &str, msg: &str) {
        let timestamp = humantime::format_rfc3339_seconds(std::time::SystemTime::now());
        let _ = self
            .sender
            .send(format!("[{}] [{}] [SUPERVISOR] {}", timestamp, id, msg))
            .await;
    }
}
