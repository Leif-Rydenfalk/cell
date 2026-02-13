// SPDX-License-Identifier: MIT
// cell-sdk/src/gpu_shared.rs
//! Triple-buffered shared GPU memory with lock-free handoff

use crate::shm_fd::{create_dma_buf, recv_fd, send_fd};
use anyhow::{Context, Result};
use std::mem::size_of;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use tokio::net::UnixStream;

/// Header for a shared GPU buffer with triple buffering
#[repr(C, align(64))]
pub struct SharedGpuBufferHeader {
    /// Write ready flags for each slot (0 = free, 1 = ready)
    pub write_ready: [AtomicU8; 3],
    /// Frame indices for each slot
    pub frames: [AtomicU64; 3],
    /// Current read slot (written by consumer)
    pub read_slot: AtomicU64,
    /// Current write slot (written by producer)
    pub write_slot: AtomicU64,
    /// Size of the buffer
    pub size: u64,
    _pad: [u8; 16], // Pad to 64 bytes
}

impl SharedGpuBufferHeader {
    pub fn new(size: u64) -> Self {
        Self {
            write_ready: Default::default(),
            frames: Default::default(),
            read_slot: AtomicU64::new(0),
            write_slot: AtomicU64::new(0),
            size,
            _pad: [0; 16],
        }
    }
}

/// A GPU buffer shared between a producer cell and the renderer
pub struct SharedGpuBuffer {
    /// Memory-mapped file descriptor
    mmap: memmap2::MmapMut,
    /// Header pointer
    header: NonNull<SharedGpuBufferHeader>,
    /// Data pointer (starts after header)
    data: NonNull<u8>,
    /// Size of data region per slot
    slot_size: usize,
    /// Whether this is the producer or consumer side
    is_producer: bool,
}

unsafe impl Send for SharedGpuBuffer {}
unsafe impl Sync for SharedGpuBuffer {}

impl SharedGpuBuffer {
    /// Create a new shared GPU buffer (producer side)
    pub async fn create(size: usize, slot_count: usize) -> Result<(Self, RawFd)> {
        assert!(
            slot_count >= 2,
            "Need at least 2 slots for triple buffering"
        );

        let total_size = size_of::<SharedGpuBufferHeader>() + size * slot_count;
        let fd = create_dma_buf(total_size)?;

        // Memory map
        let mut mmap = unsafe {
            memmap2::MmapOptions::new()
                .len(total_size)
                .map_mut(&std::fs::File::from_raw_fd(fd))?
        };

        let ptr = mmap.as_mut_ptr();

        // Initialize header
        let header = unsafe {
            let h = &mut *(ptr as *mut SharedGpuBufferHeader);
            *h = SharedGpuBufferHeader::new(size as u64);
            NonNull::new_unchecked(ptr as *mut SharedGpuBufferHeader)
        };

        let data = unsafe { NonNull::new_unchecked(ptr.add(size_of::<SharedGpuBufferHeader>())) };

        Ok((
            Self {
                mmap,
                header,
                data,
                slot_size: size,
                is_producer: true,
            },
            fd,
        ))
    }

    /// Attach to an existing shared GPU buffer (consumer side)
    pub unsafe fn attach(fd: RawFd, is_producer: bool) -> Result<Self> {
        let file = std::fs::File::from_raw_fd(fd);
        let mmap = memmap2::MmapOptions::new().map_mut(&file)?;
        let ptr = mmap.as_mut_ptr();

        Ok(Self {
            mmap,
            header: NonNull::new_unchecked(ptr as *mut SharedGpuBufferHeader),
            data: NonNull::new_unchecked(ptr.add(size_of::<SharedGpuBufferHeader>())),
            slot_size: unsafe { (*ptr.cast::<SharedGpuBufferHeader>()).size as usize },
            is_producer,
        })
    }

    /// Get the current write slot (producer only)
    pub fn begin_write(&self) -> Result<GpuWriteSlot<'_>> {
        assert!(self.is_producer, "Only producer can write");

        let header = unsafe { self.header.as_ref() };
        let write_slot = header.write_slot.fetch_add(1, Ordering::AcqRel) % 3;

        // Wait for slot to be free
        while header.write_ready[write_slot as usize].load(Ordering::Acquire) != 0 {
            std::hint::spin_loop();
        }

        let frame = header.frames[write_slot as usize].load(Ordering::Acquire);
        let slot_ptr = unsafe { self.data.as_ptr().add(write_slot as usize * self.slot_size) };

        Ok(GpuWriteSlot {
            buffer: self,
            slot: write_slot,
            frame,
            ptr: slot_ptr,
            size: self.slot_size,
        })
    }

    /// Commit a write slot (producer only)
    pub fn end_write(&self, slot: u64, frame: u64) {
        assert!(self.is_producer);
        let header = unsafe { self.header.as_ref() };
        header.frames[slot as usize].store(frame, Ordering::Release);
        header.write_ready[slot as usize].store(1, Ordering::Release);
    }

    /// Get the current read slot (consumer only)
    pub fn begin_read(&self) -> Option<GpuReadSlot<'_>> {
        assert!(!self.is_producer, "Consumer only");

        let header = unsafe { self.header.as_ref() };
        let current_frame = header.read_slot.load(Ordering::Acquire);

        // Find the newest ready slot
        for i in 0..3 {
            let slot = (current_frame.wrapping_add(i) % 3) as usize;
            if header.write_ready[slot].load(Ordering::Acquire) == 1 {
                let frame = header.frames[slot].load(Ordering::Acquire);
                let ptr = unsafe { self.data.as_ptr().add(slot * self.slot_size) };

                return Some(GpuReadSlot {
                    buffer: self,
                    slot: slot as u64,
                    frame,
                    ptr,
                    size: self.slot_size,
                });
            }
        }
        None
    }

    /// Release a read slot (consumer only)
    pub fn end_read(&self, slot: u64) {
        assert!(!self.is_producer);
        let header = unsafe { self.header.as_ref() };
        header.write_ready[slot as usize].store(0, Ordering::Release);
        header.read_slot.fetch_add(1, Ordering::Release);
    }

    /// Send the FD to another cell
    pub async fn send_to(&self, stream: &mut UnixStream) -> Result<()> {
        let fd = self.mmap.as_raw_fd();
        send_fd(stream, fd).await
    }
}

/// A write slot (producer side)
pub struct GpuWriteSlot<'a> {
    buffer: &'a SharedGpuBuffer,
    slot: u64,
    frame: u64,
    ptr: *mut u8,
    size: usize,
}

impl<'a> GpuWriteSlot<'a> {
    pub fn data_mut(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.size) }
    }

    pub fn frame(&self) -> u64 {
        self.frame
    }
    pub fn slot(&self) -> u64 {
        self.slot
    }

    pub fn commit(self, frame: u64) {
        self.buffer.end_write(self.slot, frame);
        std::mem::forget(self);
    }
}

impl<'a> Drop for GpuWriteSlot<'a> {
    fn drop(&mut self) {
        // Slot not committed - mark as free anyway to avoid leaks
        let header = unsafe { self.buffer.header.as_ref() };
        header.write_ready[self.slot as usize].store(0, Ordering::Release);
    }
}

/// A read slot (consumer side)
pub struct GpuReadSlot<'a> {
    buffer: &'a SharedGpuBuffer,
    slot: u64,
    frame: u64,
    ptr: *mut u8,
    size: usize,
}

impl<'a> GpuReadSlot<'a> {
    pub fn data(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.size) }
    }

    pub fn frame(&self) -> u64 {
        self.frame
    }
    pub fn slot(&self) -> u64 {
        self.slot
    }
}

impl<'a> Drop for GpuReadSlot<'a> {
    fn drop(&mut self) {
        self.buffer.end_read(self.slot);
    }
}
