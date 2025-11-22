use anyhow::Result;
use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{ChildStderr, ChildStdout};
use tokio::task;

const MAX_LOG_SIZE: u64 = 10 * 1024 * 1024; // 10 MB
const BACKUP_COUNT: usize = 5;

pub struct Vacuole {
    sender: mpsc::Sender<String>,
    _handle: Option<thread::JoinHandle<()>>,
}

impl Vacuole {
    pub async fn new(log_path: PathBuf) -> Result<Self> {
        if let Some(p) = log_path.parent() {
            tokio::fs::create_dir_all(p).await?;
        }

        let (tx, rx) = mpsc::channel::<String>();
        let path_clone = log_path.clone();

        let handle = thread::spawn(move || {
            // 1. Initial Open
            let mut file = match OpenOptions::new()
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

            // Track size manually to avoid syscalls on every line
            let mut current_size = file.metadata().map(|m| m.len()).unwrap_or(0);
            let mut writer = BufWriter::new(file);

            while let Ok(line) = rx.recv() {
                let bytes = line.as_bytes();
                let len = bytes.len() as u64 + 1; // +1 for newline

                // 2. Check Rotation
                if current_size + len > MAX_LOG_SIZE {
                    // Flush and Drop current writer to close the file handle
                    let _ = writer.flush();
                    drop(writer);

                    // Perform Rotation (log -> log.1, etc)
                    if let Err(e) = Self::rotate(&path_clone) {
                        eprintln!("[VACUOLE] Rotation failed: {}", e);
                    }

                    // Re-Open
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

                // 3. Write
                if writer.write_all(bytes).is_err() {
                    break;
                }
                if writer.write_all(b"\n").is_err() {
                    break;
                }
                if writer.flush().is_err() {
                    break;
                }

                current_size += len;
            }
        });

        Ok(Self {
            sender: tx,
            _handle: Some(handle),
        })
    }

    /// Synchronous rotation logic (safe to run in the OS thread)
    fn rotate(path: &Path) -> std::io::Result<()> {
        // log.4 -> log.5
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
                // On Windows, you can't rename over an existing file usually,
                // but on Linux (POSIX), atomic rename replaces the target.
                // To be safe cross-platform, remove dst first.
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
                    let _ = tx.send(format!("[{}] {}", id, line));
                }
            });
        }

        if let Some(err) = stderr {
            let tx = self.sender.clone();
            let id = id.clone();
            task::spawn(async move {
                let mut reader = BufReader::new(err).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    let _ = tx.send(format!("[{}] [ERR] {}", id, line));
                }
            });
        }
    }

    pub async fn log(&self, id: &str, msg: &str) {
        let timestamp = humantime::format_rfc3339_seconds(std::time::SystemTime::now());
        let _ = self
            .sender
            .send(format!("[{}] [{}] [SUPERVISOR] {}", timestamp, id, msg));
    }
}
