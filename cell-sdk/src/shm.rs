use anyhow::{Context, Result};
use memmap2::MmapMut;
use nix::sys::memfd::{memfd_create, MemFdCreateFlag};
use std::ffi::CString;
use std::fs::File;
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::sync::atomic::{AtomicUsize, Ordering};

const SHM_SIZE: usize = 1024 * 1024; // 1MB Ring Buffer
const HEADER_SIZE: usize = 128; // Reserved space for Atomic Pointers

/// The memory layout inside the Shared Region
/// [ ReadHead(8) | WriteHead(8) | ...Padding... | Data Buffer... ]
struct RingControl {
    read_head: *mut AtomicUsize,
    write_head: *mut AtomicUsize,
    data_ptr: *mut u8,
    cap: usize,
}

pub struct GapJunction {
    mmap: MmapMut,
    control: RingControl,
    // Keep file open so FD stays valid
    _file: File,
}

// Safety: We are essentially building a manual synchronization primitive
unsafe impl Send for GapJunction {}

impl GapJunction {
    /// Create a new anonymous shared memory file (Parent/Allocator side)
    pub fn create() -> Result<(Self, RawFd)> {
        let name = CString::new("cell_gap")?;
        let fd = memfd_create(&name, MemFdCreateFlag::MFD_CLOEXEC)?;
        let file = unsafe { File::from_raw_fd(fd) };

        file.set_len(SHM_SIZE as u64)?;

        let mut mmap = unsafe { MmapMut::map_mut(&file)? };

        // Zero out headers
        mmap[0..16].fill(0);

        let control = unsafe { Self::layout(mmap.as_mut_ptr()) };

        let junction = Self {
            mmap,
            control,
            _file: file,
        };

        Ok((junction, fd))
    }

    /// Open an existing shared memory file from an FD (Child/Consumer side)
    pub unsafe fn open(fd: RawFd) -> Result<Self> {
        let file = File::from_raw_fd(fd);
        let mut mmap = MmapMut::map_mut(&file)?;
        let control = Self::layout(mmap.as_mut_ptr());

        Ok(Self {
            mmap,
            control,
            _file: file,
        })
    }

    unsafe fn layout(base: *mut u8) -> RingControl {
        RingControl {
            read_head: base as *mut AtomicUsize,
            write_head: (base as usize + 8) as *mut AtomicUsize,
            data_ptr: (base as usize + HEADER_SIZE) as *mut u8,
            cap: SHM_SIZE - HEADER_SIZE,
        }
    }

    /// Write data to ring buffer. Returns bytes written.
    pub fn write(&self, data: &[u8]) -> usize {
        let read = unsafe { (*self.control.read_head).load(Ordering::Acquire) };
        let write = unsafe { (*self.control.write_head).load(Ordering::Acquire) };
        let cap = self.control.cap;

        // Calculate free space (Naive implementation)
        let free = if read <= write {
            cap - (write - read) - 1
        } else {
            read - write - 1
        };

        let to_write = std::cmp::min(data.len(), free);
        if to_write == 0 {
            return 0;
        }

        unsafe {
            let offset = write % cap;
            // Handle wrap-around copy
            let first_chunk = std::cmp::min(to_write, cap - offset);
            std::ptr::copy_nonoverlapping(
                data.as_ptr(),
                self.control.data_ptr.add(offset),
                first_chunk,
            );

            if first_chunk < to_write {
                std::ptr::copy_nonoverlapping(
                    data.as_ptr().add(first_chunk),
                    self.control.data_ptr,
                    to_write - first_chunk,
                );
            }

            // Advance Write Head
            (*self.control.write_head).store((write + to_write) % cap, Ordering::Release);
        }
        to_write
    }

    /// Read data from ring buffer.
    pub fn read(&self, buf: &mut [u8]) -> usize {
        let read = unsafe { (*self.control.read_head).load(Ordering::Acquire) };
        let write = unsafe { (*self.control.write_head).load(Ordering::Acquire) };

        if read == write {
            return 0;
        } // Empty

        let cap = self.control.cap;
        let available = if write >= read {
            write - read
        } else {
            cap - read + write
        };

        let to_read = std::cmp::min(buf.len(), available);

        unsafe {
            let offset = read % cap;
            let first_chunk = std::cmp::min(to_read, cap - offset);

            std::ptr::copy_nonoverlapping(
                self.control.data_ptr.add(offset),
                buf.as_mut_ptr(),
                first_chunk,
            );

            if first_chunk < to_read {
                std::ptr::copy_nonoverlapping(
                    self.control.data_ptr,
                    buf.as_mut_ptr().add(first_chunk),
                    to_read - first_chunk,
                );
            }

            // Advance Read Head
            (*self.control.read_head).store((read + to_read) % cap, Ordering::Release);
        }
        to_read
    }
}
