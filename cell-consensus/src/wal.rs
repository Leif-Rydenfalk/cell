// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::Result;
use rkyv::{Archive, Serialize, Deserialize};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use tracing::{warn, error};

#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq)]
#[archive(check_bytes)]
pub enum LogEntry {
    Command(Vec<u8>),
    ConfigChange,
}

pub struct WriteAheadLog {
    file: File,
}

impl WriteAheadLog {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(path)?;
        Ok(Self { file })
    }

    pub fn append_batch(&mut self, entries: &[LogEntry]) -> Result<()> {
        for entry in entries {
            self.write_entry_no_sync(entry)?;
        }
        self.file.sync_data()?;
        Ok(())
    }

    fn write_entry_no_sync(&mut self, entry: &LogEntry) -> Result<()> {
        let bytes = rkyv::to_bytes::<_, 256>(entry)?.into_vec();
        let len = bytes.len() as u64;
        let crc = crc32fast::hash(&bytes);

        let mut buffer = Vec::with_capacity(8 + 4 + bytes.len());
        buffer.extend_from_slice(&len.to_le_bytes());
        buffer.extend_from_slice(&crc.to_le_bytes());
        buffer.extend_from_slice(&bytes);

        self.file.seek(SeekFrom::End(0))?;
        self.file.write_all(&buffer)?;
        
        Ok(())
    }

    pub fn read_all(&mut self) -> Result<Vec<LogEntry>> {
        const MAX_ENTRY_SIZE: u64 = 100 * 1024 * 1024;

        let mut entries = Vec::new();
        self.file.seek(SeekFrom::Start(0))?;
        let mut len_buf = [0u8; 8];
        let mut crc_buf = [0u8; 4];

        loop {
            if self.file.read_exact(&mut len_buf).is_err() { break; }
            let len = u64::from_le_bytes(len_buf);

            if len > MAX_ENTRY_SIZE {
                warn!("[WAL] Skipping corrupted entry");
                break;
            }

            if self.file.read_exact(&mut crc_buf).is_err() { break; }
            let expected_crc = u32::from_le_bytes(crc_buf);

            let mut buf = vec![0u8; len as usize];
            if self.file.read_exact(&mut buf).is_err() { break; }

            let actual_crc = crc32fast::hash(&buf);
            if actual_crc != expected_crc {
                error!("[WAL] CRC mismatch");
                break;
            }

            if let Ok(archived) = rkyv::check_archived_root::<LogEntry>(&buf) {
                 if let Ok(e) = archived.deserialize(&mut rkyv::Infallible) {
                     entries.push(e);
                 }
            } else {
                error!("[WAL] Failed to deserialize entry");
                break;
            }
        }
        Ok(entries)
    }
}