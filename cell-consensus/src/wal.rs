use anyhow::Result; // Removed unused Context
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
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

    pub fn append(&mut self, entry: &LogEntry) -> Result<()> {
        self.write_entry_to_buffer(entry)?;
        self.file.sync_data()?;
        Ok(())
    }

    // NEW: Batch optimization
    pub fn append_batch(&mut self, entries: &[LogEntry]) -> Result<()> {
        for entry in entries {
            self.write_entry_to_buffer(entry)?;
        }
        // Sync only once for the whole batch
        self.file.sync_data()?;
        Ok(())
    }

    fn write_entry_to_buffer(&mut self, entry: &LogEntry) -> Result<()> {
        let bytes = bincode::serialize(entry)?;
        let len = bytes.len() as u64;
        let crc = crc32fast::hash(&bytes);

        self.file.seek(SeekFrom::End(0))?;
        self.file.write_all(&len.to_le_bytes())?;
        self.file.write_all(&crc.to_le_bytes())?;
        self.file.write_all(&bytes)?;
        Ok(())
    }

    pub fn read_all(&mut self) -> Result<Vec<LogEntry>> {
        let mut entries = Vec::new();
        self.file.seek(SeekFrom::Start(0))?;
        let mut len_buf = [0u8; 8];
        let mut crc_buf = [0u8; 4];

        loop {
            if self.file.read_exact(&mut len_buf).is_err() {
                break;
            }
            let len = u64::from_le_bytes(len_buf);
            if self.file.read_exact(&mut crc_buf).is_err() {
                break;
            }
            let _crc = u32::from_le_bytes(crc_buf);
            let mut buf = vec![0u8; len as usize];
            if self.file.read_exact(&mut buf).is_err() {
                break;
            }

            if let Ok(e) = bincode::deserialize(&buf) {
                entries.push(e);
            }
        }
        Ok(entries)
    }
}
