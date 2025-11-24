use crate::LogEntry;
use anyhow::{Context, Result};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

/// A simple append-only Write Ahead Log.
/// Format: [Length: u64][CRC: u32][Payload: Bytes]
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
            .context("Failed to open WAL file")?;

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
        self.file.sync_data()?; // Fsync for durability
        Ok(())
    }

    pub fn read_all(&mut self) -> Result<Vec<LogEntry>> {
        let mut entries = Vec::new();
        self.file.seek(SeekFrom::Start(0))?;

        let mut len_buf = [0u8; 8];
        let mut crc_buf = [0u8; 4];

        loop {
            if self.file.read_exact(&mut len_buf).is_err() {
                break; // EOF
            }
            let len = u64::from_le_bytes(len_buf);

            if self.file.read_exact(&mut crc_buf).is_err() {
                break; // Corrupt/Truncated
            }
            let _expected_crc = u32::from_le_bytes(crc_buf);

            let mut payload = vec![0u8; len as usize];
            self.file.read_exact(&mut payload)?;

            // In prod: Verify CRC here

            let entry: LogEntry = bincode::deserialize(&payload)?;
            entries.push(entry);
        }

        Ok(entries)
    }
}
