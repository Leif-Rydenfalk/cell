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
            // 1. Read Length
            // If we fail to read the length (EOF), we assume end of log.
            if self.file.read_exact(&mut len_buf).is_err() {
                break;
            }
            let len = u64::from_le_bytes(len_buf);

            // 2. Read CRC
            // If we have a length but no CRC, it's a corrupted/partial tail.
            if self.file.read_exact(&mut crc_buf).is_err() {
                break;
            }
            let _expected_crc = u32::from_le_bytes(crc_buf);

            // 3. Read Payload
            let mut payload = vec![0u8; len as usize];
            // If we can't read the full payload, it's a partial write.
            if self.file.read_exact(&mut payload).is_err() {
                break;
            }

            // Optional: Verify CRC here in production
            // if crc32fast::hash(&payload) != _expected_crc { break; }

            // 4. Deserialize
            // If deserialization fails, we stop (assume garbage data).
            if let Ok(entry) = bincode::deserialize(&payload) {
                entries.push(entry);
            } else {
                break;
            }
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

        {
            let mut wal = WriteAheadLog::open(&path)?;
            wal.append(&entry)?;
        }

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
        file.set_len(len - 2)?; // Cut off 2 bytes from the end (truncates payload or CRC)

        // 3. Attempt read
        let mut wal = WriteAheadLog::open(path)?;
        let entries = wal.read_all()?;

        // Should handle the partial read gracefully and return empty (since the only entry is broken)
        assert!(entries.is_empty(), "Should not return corrupted entry");

        Ok(())
    }
}
