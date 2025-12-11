// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::Result;
use serde::{Serialize, Deserialize};
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum LogEntry {
    Command { term: u64, data: Vec<u8> },
    NoOp { term: u64 },
}

impl LogEntry {
    pub fn term(&self) -> u64 {
        match self {
            LogEntry::Command { term, .. } => *term,
            LogEntry::NoOp { term } => *term,
        }
    }
}

/// Metadata stored in a separate file (CurrentTerm, VotedFor)
#[derive(Serialize, Deserialize, Default, Debug, Clone)]
pub struct HardState {
    pub current_term: u64,
    pub voted_for: Option<u64>,
}

pub struct WriteAheadLog {
    path: PathBuf,
    state_path: PathBuf,
    entries: Vec<LogEntry>, // In-memory cache of log for fast reads
    hard_state: HardState,
}

impl WriteAheadLog {
    pub fn open(storage_path: &Path) -> Result<Self> {
        if let Some(parent) = storage_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let state_path = storage_path.with_extension("state");
        
        let mut wal = Self {
            path: storage_path.to_path_buf(),
            state_path,
            entries: Vec::new(),
            hard_state: HardState::default(),
        };

        wal.recover()?;
        Ok(wal)
    }

    fn recover(&mut self) -> Result<()> {
        // 1. Recover Hard State
        if self.state_path.exists() {
            let file = File::open(&self.state_path)?;
            let reader = BufReader::new(file);
            self.hard_state = bincode::deserialize_from(reader)
                .unwrap_or_else(|_| HardState::default());
        }

        // 2. Recover Log Entries
        if self.path.exists() {
            let file = File::open(&self.path)?;
            let len = file.metadata()?.len();
            if len > 0 {
                let mut reader = BufReader::new(file);
                while let Ok(entry) = bincode::deserialize_from::<_, LogEntry>(&mut reader) {
                    self.entries.push(entry);
                }
            }
        }

        Ok(())
    }

    pub fn save_hard_state(&mut self, term: u64, voted_for: Option<u64>) -> Result<()> {
        self.hard_state.current_term = term;
        self.hard_state.voted_for = voted_for;
        
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&self.state_path)?;
        
        bincode::serialize_into(BufWriter::new(file), &self.hard_state)?;
        Ok(())
    }

    pub fn hard_state(&self) -> HardState {
        self.hard_state.clone()
    }

    pub fn append(&mut self, entry: LogEntry) -> Result<u64> {
        let file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&self.path)?;
        
        let mut writer = BufWriter::new(file);
        bincode::serialize_into(&mut writer, &entry)?;
        writer.flush()?;

        self.entries.push(entry);
        Ok(self.entries.len() as u64) // Index starts at 1 conceptually, but 0-indexed vec + 1 = len
    }

    pub fn get_entry(&self, index: u64) -> Option<LogEntry> {
        if index == 0 || index > self.entries.len() as u64 {
            return None;
        }
        Some(self.entries[(index - 1) as usize].clone())
    }

    pub fn last_index(&self) -> u64 {
        self.entries.len() as u64
    }

    pub fn last_log_info(&self) -> (u64, u64) {
        let idx = self.last_index();
        let term = self.get_entry(idx).map(|e| e.term()).unwrap_or(0);
        (idx, term)
    }

    pub fn truncate_suffix(&mut self, index: u64) -> Result<()> {
        // Delete everything from index onwards
        // Used when conflict is found
        if index > self.entries.len() as u64 {
            return Ok(());
        }

        self.entries.truncate((index - 1) as usize);
        
        // Rewrite disk file (Simplified approach: Rewrite whole log)
        // In production, you'd use `ftruncate` but serde framing makes that tricky without index.
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&self.path)?;
            
        let mut writer = BufWriter::new(file);
        for entry in &self.entries {
            bincode::serialize_into(&mut writer, entry)?;
        }
        writer.flush()?;
        
        Ok(())
    }

    pub fn get_entries_from(&self, start_idx: u64) -> Vec<LogEntry> {
        if start_idx == 0 || start_idx > self.entries.len() as u64 {
            return Vec::new();
        }
        self.entries[(start_idx - 1) as usize..].to_vec()
    }
}