// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::Result;
use std::path::Path;
use tokio::fs::File;
use tokio::io::{AsyncWriteExt, AsyncReadExt};
use rkyv::{Archive, Serialize, Deserialize};

#[derive(Archive, Serialize, Deserialize)]
#[archive(check_bytes)]
pub struct Snapshot {
    pub last_included_index: u64,
    pub last_included_term: u64,
    pub data: Vec<u8>,
}

impl Snapshot {
    pub async fn save(&self, path: &Path) -> Result<()> {
        let bytes = rkyv::to_bytes::<_, 1024>(self)?.into_vec();
        let mut file = File::create(path).await?;
        file.write_all(&bytes).await?;
        file.sync_all().await?;
        Ok(())
    }

    pub async fn load(path: &Path) -> Result<Self> {
        let mut file = File::open(path).await?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).await?;

        let archived = rkyv::check_archived_root::<Snapshot>(&bytes).map_err(|e| anyhow::anyhow!("Snapshot corrupted: {}", e))?;
        Ok(archived.deserialize(&mut rkyv::Infallible)?)
    }
}