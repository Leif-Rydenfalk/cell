// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::Result;
use std::path::Path;
use crate::wal::WriteAheadLog;

pub struct Compactor {
    snapshot_threshold: u64,
}

impl Compactor {
    pub fn new(snapshot_threshold: u64) -> Self {
        Self { snapshot_threshold }
    }

    pub async fn maybe_compact(
        &self,
        wal_path: &Path,
        current_index: u64,
    ) -> Result<bool> {
        if current_index < self.snapshot_threshold {
            return Ok(false);
        }

        // Open current WAL
        let mut wal = WriteAheadLog::open(wal_path)?;
        
        // Read all entries
        let entries = wal.read_all()?;
        
        // Keep only last N entries (simplistic compaction strategy)
        let keep_count = (self.snapshot_threshold / 2) as usize;
        let to_keep = if entries.len() > keep_count {
            &entries[entries.len() - keep_count..]
        } else {
            &entries[..]
        };

        // Write new compacted WAL
        let temp_path = wal_path.with_extension("tmp");
        if temp_path.exists() {
            std::fs::remove_file(&temp_path)?;
        }
        
        let mut new_wal = WriteAheadLog::open(&temp_path)?;
        new_wal.append_batch(to_keep)?;

        // Atomic swap
        std::fs::rename(&temp_path, wal_path)?;
        
        Ok(true)
    }
}