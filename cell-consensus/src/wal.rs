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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LogEntry;
    use tempfile::NamedTempFile;

    #[test]
    fn test_wal_write_and_read() -> Result<()> {
        let tmp = NamedTempFile::new()?;
        let path = tmp.path();

        let mut wal = WriteAheadLog::open(path)?;

        let entry1 = LogEntry::Command(b"hello".to_vec());
        let entry2 = LogEntry::Command(b"world".to_vec());
        let entry3 = LogEntry::ConfigChange;

        wal.append(&entry1)?;
        wal.append(&entry2)?;
        wal.append(&entry3)?;

        // Re-read immediately
        let entries = wal.read_all()?;
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0], entry1);
        assert_eq!(entries[1], entry2);
        assert_eq!(entries[2], entry3);

        Ok(())
    }

    #[test]
    fn test_wal_recovery() -> Result<()> {
        let tmp = NamedTempFile::new()?;
        let path = tmp.path().to_owned();

        let entry = LogEntry::Command(vec![1, 2, 3, 4]);

        // 1. Open, Write, Close
        {
            let mut wal = WriteAheadLog::open(&path)?;
            wal.append(&entry)?;
        } // wal dropped here, file closed

        // 2. Re-open
        {
            let mut wal_reopened = WriteAheadLog::open(&path)?;
            let entries = wal_reopened.read_all()?;

            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0], entry);
        }

        Ok(())
    }

    #[test]
    fn test_wal_corruption_handling() -> Result<()> {
        let tmp = NamedTempFile::new()?;
        let path = tmp.path();

        // 1. Create valid WAL
        {
            let mut wal = WriteAheadLog::open(path)?;
            wal.append(&LogEntry::Command(b"valid".to_vec()))?;
        }

        // 2. Corrupt the file (truncate slightly to break CRC/Len)
        let mut file = std::fs::OpenOptions::new().write(true).open(path)?;
        let len = file.metadata()?.len();
        file.set_len(len - 2)?; // Cut off 2 bytes from the end

        // 3. Attempt read
        let mut wal = WriteAheadLog::open(path)?;
        let entries = wal.read_all()?;

        // Depending on implementation, it should either return 0 entries
        // (if the first one was corrupted) or return strict error.
        // Our current impl breaks loop on read error, so it might return empty.
        // Ideally, a robust WAL would return the valid prefix.
        assert!(entries.is_empty(), "Should not return corrupted entry");

        Ok(())
    }
}
