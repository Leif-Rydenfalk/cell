// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::Result;
use rkyv::{Archive, Serialize, Deserialize};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use tracing::{warn, error, info};

const MAGIC: u32 = 0xCE11_DA7A; // CELL DATA

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

        // Header: MAGIC (4) + LEN (8) + CRC (4)
        let mut buffer = Vec::with_capacity(4 + 8 + 4 + bytes.len());
        buffer.extend_from_slice(&MAGIC.to_le_bytes());
        buffer.extend_from_slice(&len.to_le_bytes());
        buffer.extend_from_slice(&crc.to_le_bytes());
        buffer.extend_from_slice(&bytes);

        self.file.seek(SeekFrom::End(0))?;
        self.file.write_all(&buffer)?;
        
        Ok(())
    }

    pub fn read_all(&mut self) -> Result<Vec<LogEntry>> {
        const MAX_ENTRY_SIZE: u64 = 100 * 1024 * 1024; // 100MB sanity limit

        let mut entries = Vec::new();
        self.file.seek(SeekFrom::Start(0))?;
        
        let mut magic_buf = [0u8; 4];
        let mut len_buf = [0u8; 8];
        let mut crc_buf = [0u8; 4];

        loop {
            // 1. Read Magic
            if self.file.read_exact(&mut magic_buf).is_err() { break; } // EOF usually
            let magic = u32::from_le_bytes(magic_buf);

            if magic != MAGIC {
                warn!("[WAL] Corruption detected (Invalid Magic: 0x{:x}). Entering scan mode...", magic);
                // Resync: Scan byte-by-byte for next magic
                if let Err(_) = self.scan_for_next_magic() {
                    break; // EOF reached during scan or hard fail
                }
                continue;
            }

            // 2. Read Len
            if self.file.read_exact(&mut len_buf).is_err() { break; }
            let len = u64::from_le_bytes(len_buf);

            if len > MAX_ENTRY_SIZE {
                warn!("[WAL] Skipping corrupted entry (Size {} > Limit). Resyncing.", len);
                let _ = self.scan_for_next_magic();
                continue;
            }

            // 3. Read CRC
            if self.file.read_exact(&mut crc_buf).is_err() { break; }
            let expected_crc = u32::from_le_bytes(crc_buf);

            // 4. Read Data
            let mut buf = vec![0u8; len as usize];
            if self.file.read_exact(&mut buf).is_err() { 
                warn!("[WAL] Unexpected EOF reading payload. Truncating?");
                break; 
            }

            let actual_crc = crc32fast::hash(&buf);
            if actual_crc != expected_crc {
                warn!("[WAL] CRC mismatch. Expected: {:x}, Got: {:x}. Entry invalid.", expected_crc, actual_crc);
                let _ = self.scan_for_next_magic();
                continue;
            }

            if let Ok(archived) = rkyv::check_archived_root::<LogEntry>(&buf) {
                 if let Ok(e) = archived.deserialize(&mut rkyv::Infallible) {
                     entries.push(e);
                 }
            } else {
                error!("[WAL] Failed to deserialize entry despite valid CRC");
            }
        }
        Ok(entries)
    }

    fn scan_for_next_magic(&mut self) -> Result<()> {
        let mut window = [0u8; 4];
        if self.file.read_exact(&mut window).is_err() { return Err(anyhow::anyhow!("EOF")); }
        
        loop {
            if u32::from_le_bytes(window) == MAGIC {
                self.file.seek(SeekFrom::Current(-4))?;
                info!("[WAL] Resynced at offset {}", self.file.stream_position()?);
                return Ok(());
            }

            window[0] = window[1];
            window[1] = window[2];
            window[2] = window[3];
            
            let mut byte = [0u8; 1];
            if self.file.read_exact(&mut byte).is_err() {
                return Err(anyhow::anyhow!("EOF"));
            }
            window[3] = byte[0];
        }
    }
}