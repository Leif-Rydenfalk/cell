// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use cell_model::protocol::GENOME_REQUEST;
use crate::resolve_socket_dir;

pub async fn scan_local_sockets() -> Vec<String> {
    let mut names = vec![];
    let path = resolve_socket_dir();
    if !path.exists() {
        return names;
    }

    if let Ok(mut entries) = tokio::fs::read_dir(path).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if let Some(ext) = path.extension() {
                if ext == "sock" {
                    if let Some(stem) = path.file_stem() {
                        names.push(stem.to_string_lossy().to_string());
                    }
                }
            }
        }
    }
    names
}

pub async fn probe_unix_socket(path: &PathBuf) -> Option<Duration> {
    let start = Instant::now();
    let mut stream = tokio::net::UnixStream::connect(path).await.ok()?;

    let req_len = GENOME_REQUEST.len() as u32;
    stream.write_all(&req_len.to_le_bytes()).await.ok()?;
    stream.write_all(GENOME_REQUEST).await.ok()?;

    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await.ok()?;

    Some(start.elapsed())
}