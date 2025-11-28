use anyhow::Result; // Removed unused Context
use memmap2::MmapMut;
#[cfg(target_os = "linux")]
use nix::sys::memfd::{memfd_create, MemFdCreateFlag};
#[cfg(target_os = "linux")]
use std::ffi::CString;
use std::fs::File;
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::sync::atomic::{AtomicUsize, Ordering};

const SHM_SIZE: usize = 4 * 1024 * 1024;
const HEADER_SIZE: usize = 128;

struct RingLayout {
    read: *mut AtomicUsize,
    write: *mut AtomicUsize,
    data: *mut u8,
    cap: usize,
}

pub struct GapJunction {
    #[allow(dead_code)] // mmap needs to be kept alive even if not read directly
    mmap: MmapMut,
    layout: RingLayout,
    #[allow(dead_code)]
    _file: File,
}

unsafe impl Send for GapJunction {}

impl GapJunction {
    #[cfg(target_os = "linux")]
    pub fn forge() -> Result<(Self, RawFd)> {
        let name = CString::new("cell_gap")?;
        let owned_fd = memfd_create(&name, MemFdCreateFlag::MFD_CLOEXEC)?;
        let file = File::from(owned_fd);
        file.set_len(SHM_SIZE as u64)?;

        let mut mmap = unsafe { MmapMut::map_mut(&file)? };
        mmap[0..16].fill(0);

        let layout = unsafe { Self::get_layout(mmap.as_mut_ptr()) };
        let fd_out = file.as_raw_fd();

        Ok((
            Self {
                mmap,
                layout,
                _file: file,
            },
            fd_out,
        ))
    }

    pub unsafe fn attach(fd: RawFd) -> Result<Self> {
        let file = File::from_raw_fd(fd);
        let mut mmap = MmapMut::map_mut(&file)?;
        let layout = Self::get_layout(mmap.as_mut_ptr());
        Ok(Self {
            mmap,
            layout,
            _file: file,
        })
    }

    unsafe fn get_layout(ptr: *mut u8) -> RingLayout {
        RingLayout {
            read: ptr as *mut AtomicUsize,
            write: (ptr as usize + 8) as *mut AtomicUsize,
            data: (ptr as usize + HEADER_SIZE) as *mut u8,
            cap: SHM_SIZE - HEADER_SIZE,
        }
    }

    // ... write/read methods remain unchanged ...
    pub fn write(&self, src: &[u8]) -> usize {
        let read = unsafe { (*self.layout.read).load(Ordering::Acquire) };
        let write = unsafe { (*self.layout.write).load(Ordering::Acquire) };
        let cap = self.layout.cap;

        let free = if read <= write {
            cap - (write - read) - 1
        } else {
            read - write - 1
        };
        let count = std::cmp::min(src.len(), free);
        if count == 0 {
            return 0;
        }

        unsafe {
            let offset = write % cap;
            let chunk1 = std::cmp::min(count, cap - offset);
            std::ptr::copy_nonoverlapping(src.as_ptr(), self.layout.data.add(offset), chunk1);
            if chunk1 < count {
                std::ptr::copy_nonoverlapping(
                    src.as_ptr().add(chunk1),
                    self.layout.data,
                    count - chunk1,
                );
            }
            (*self.layout.write).store((write + count) % cap, Ordering::Release);
        }
        count
    }

    pub fn read(&self, dst: &mut [u8]) -> usize {
        let read = unsafe { (*self.layout.read).load(Ordering::Acquire) };
        let write = unsafe { (*self.layout.write).load(Ordering::Acquire) };
        if read == write {
            return 0;
        }

        let cap = self.layout.cap;
        let available = if write >= read {
            write - read
        } else {
            cap - read + write
        };
        let count = std::cmp::min(dst.len(), available);

        unsafe {
            let offset = read % cap;
            let chunk1 = std::cmp::min(count, cap - offset);
            std::ptr::copy_nonoverlapping(self.layout.data.add(offset), dst.as_mut_ptr(), chunk1);
            if chunk1 < count {
                std::ptr::copy_nonoverlapping(
                    self.layout.data,
                    dst.as_mut_ptr().add(chunk1),
                    count - chunk1,
                );
            }
            (*self.layout.read).store((read + count) % cap, Ordering::Release);
        }
        count
    }
}
