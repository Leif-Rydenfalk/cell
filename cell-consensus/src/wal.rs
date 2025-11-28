use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum LogEntry {
    Command(Vec<u8>),
    ConfigChange,
}

/// Disk-based Append-Only Log
/// Format: [Len: u64][CRC: u32][Payload]
pub struct WriteAheadLog {
    file: File,
}

impl WriteAheadLog {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(path)
            .context("Failed to open WAL")?;
        Ok(Self { file })
    }

    pub fn append(&mut self, entry: &LogEntry) -> Result<()> {
        let bytes = bincode::serialize(entry)?;
        let len = bytes.len() as u64;
        let crc = crc32fast::hash(&bytes);

        self.file.seek(SeekFrom::End(0))?;
        self.file.write_all(&len.to_le_bytes())?;
        self.file.write_all(&crc.to_le_bytes())?;
        self.file.write_all(&bytes)?;
        self.file.sync_data()?; // Durability flush
        Ok(())
    }

    pub fn read_all(&mut self) -> Result<Vec<LogEntry>> {
        let mut entries = Vec::new();
        self.file.seek(SeekFrom::Start(0))?;

        let mut len_buf = [0u8; 8];
        let mut crc_buf = [0u8; 4];

        loop {
            // Check EOF by reading length
            if self.file.read_exact(&mut len_buf).is_err() {
                break;
            }
            let len = u64::from_le_bytes(len_buf);

            // Read CRC
            if self.file.read_exact(&mut crc_buf).is_err() {
                break;
            }
            let _expected = u32::from_le_bytes(crc_buf);

            // Read Payload
            let mut buf = vec![0u8; len as usize];
            if self.file.read_exact(&mut buf).is_err() {
                break;
            }

            // (Optional: Verify CRC here)

            if let Ok(entry) = bincode::deserialize(&buf) {
                entries.push(entry);
            }
        }
        Ok(entries)
    }
}
