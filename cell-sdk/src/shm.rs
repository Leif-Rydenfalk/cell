// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use cell_core::CellError;
use memmap2::MmapMut;
use rkyv::ser::serializers::AllocSerializer;
use rkyv::{Archive, Deserialize, Serialize};
use std::fs::File;
use std::marker::PhantomData;
use std::ptr;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;

pub type ShmSerializer = AllocSerializer<1024>;

const CACHE_LINE: usize = 64;
const RING_SIZE: usize = 32 * 1024 * 1024;
const DATA_OFFSET: usize = 128;
const DATA_CAPACITY: usize = RING_SIZE - DATA_OFFSET;
const PADDING_SENTINEL: u32 = 0xFFFFFFFF;
const ALIGNMENT: usize = 16;
const HEADER_SIZE: usize = std::mem::size_of::<SlotHeader>();

#[repr(C)]
struct SlotHeader {
    refcount: AtomicU32,
    len: AtomicU32,
    epoch: AtomicU64,
    generation: AtomicU64,
    owner_pid: AtomicU32,
    channel: AtomicU8,
    _pad: [u8; 3],
}

#[repr(C, align(64))]
struct RingControl {
    write_pos: AtomicU64,
    _pad1: [u8; CACHE_LINE - 8],
    read_pos: AtomicU64,
    _pad2: [u8; CACHE_LINE - 8],
}

pub struct RingBuffer {
    control: *mut RingControl,
    data: *mut u8,
    capacity: usize,
    _mmap: MmapMut,
    _file: Option<File>,
}

unsafe impl Send for RingBuffer {}
unsafe impl Sync for RingBuffer {}

impl RingBuffer {
    #[cfg(target_os = "linux")]
    pub fn create(name: &str) -> anyhow::Result<(Arc<Self>, std::os::unix::io::RawFd)> {
        use nix::sys::memfd::{memfd_create, MemFdCreateFlag};
        use std::ffi::CString;
        use std::os::unix::io::AsRawFd;

        let name_cstr = CString::new(name)?;
        let flags = MemFdCreateFlag::MFD_CLOEXEC | MemFdCreateFlag::MFD_ALLOW_SEALING;

        let owned_fd = match memfd_create(&name_cstr, flags) {
            Ok(fd) => fd,
            Err(e) => return Err(e.into()),
        };

        let file = File::from(owned_fd);
        file.set_len(RING_SIZE as u64)?;

        let raw_fd = file.as_raw_fd();
        let seals = nix::fcntl::SealFlag::F_SEAL_GROW
            | nix::fcntl::SealFlag::F_SEAL_SHRINK
            | nix::fcntl::SealFlag::F_SEAL_SEAL;
        nix::fcntl::fcntl(raw_fd, nix::fcntl::F_ADD_SEALS(seals))?;

        let mut mmap = unsafe { MmapMut::map_mut(&file)? };
        mmap[..DATA_OFFSET + 4096].fill(0);

        let control = mmap.as_mut_ptr() as *mut RingControl;
        let data = unsafe { mmap.as_mut_ptr().add(DATA_OFFSET) };

        Ok((
            Arc::new(Self {
                control,
                data,
                capacity: DATA_CAPACITY,
                _mmap: mmap,
                _file: Some(file),
            }),
            raw_fd,
        ))
    }

    #[cfg(target_os = "macos")]
    pub fn create(name: &str) -> anyhow::Result<(Arc<Self>, std::os::unix::io::RawFd)> {
        use nix::fcntl::OFlag;
        use nix::sys::mman::{shm_open, shm_unlink};
        use nix::sys::stat::Mode;
        use nix::unistd::ftruncate;
        use std::ffi::CString;
        use std::os::unix::io::AsRawFd;

        let unique_name = format!("/{}_{}", name, rand::random::<u32>());
        let name_cstr = CString::new(unique_name)?;

        let owned_fd = shm_open(
            name_cstr.as_c_str(),
            OFlag::O_CREAT | OFlag::O_RDWR | OFlag::O_EXCL,
            Mode::S_IRUSR | Mode::S_IWUSR,
        )?;

        let _ = shm_unlink(name_cstr.as_c_str());
        ftruncate(&owned_fd, RING_SIZE as i64)?;

        let file = File::from(owned_fd);
        let raw_fd = file.as_raw_fd();

        let mut mmap = unsafe { MmapMut::map_mut(&file)? };
        mmap[..DATA_OFFSET + 4096].fill(0);

        let control = mmap.as_mut_ptr() as *mut RingControl;
        let data = unsafe { mmap.as_mut_ptr().add(DATA_OFFSET) };

        Ok((
            Arc::new(Self {
                control,
                data,
                capacity: DATA_CAPACITY,
                _mmap: mmap,
                _file: Some(file),
            }),
            raw_fd,
        ))
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub unsafe fn attach(fd: std::os::unix::io::RawFd) -> anyhow::Result<Arc<Self>> {
        use std::os::unix::io::FromRawFd;
        let owned_fd = std::os::unix::io::OwnedFd::from_raw_fd(fd);
        let file = File::from(owned_fd);
        let mut mmap = MmapMut::map_mut(&file)?;

        let control = mmap.as_mut_ptr() as *mut RingControl;
        let data = mmap.as_mut_ptr().add(DATA_OFFSET);

        Ok(Arc::new(Self {
            control,
            data,
            capacity: DATA_CAPACITY,
            _mmap: mmap,
            _file: Some(file),
        }))
    }

    pub fn try_alloc(&self, exact_size: usize) -> Option<WriteSlot<'_>> {
        const MAX_ALLOC_SIZE: usize = 16 * 1024 * 1024;
        if exact_size > MAX_ALLOC_SIZE {
            return None;
        }

        let aligned_size = (exact_size + ALIGNMENT - 1) & !(ALIGNMENT - 1);
        let total_needed = HEADER_SIZE + aligned_size;

        loop {
            let write = unsafe { (*self.control).write_pos.load(Ordering::Acquire) };
            let read = unsafe { (*self.control).read_pos.load(Ordering::Acquire) };

            let used = write.wrapping_sub(read);
            if used + total_needed as u64 > self.capacity as u64 {
                return None;
            }

            let write_idx = (write % self.capacity as u64) as usize;
            let space_at_end = self.capacity - write_idx;

            let (offset, wrap_padding) = if space_at_end >= total_needed {
                (write_idx, 0)
            } else {
                if used + space_at_end as u64 + total_needed as u64 > self.capacity as u64 {
                    return None;
                }
                (0, space_at_end)
            };

            let new_write = write + wrap_padding as u64 + total_needed as u64;

            if unsafe {
                (*self.control)
                    .write_pos
                    .compare_exchange_weak(write, new_write, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
            } {
                if wrap_padding > 0 && space_at_end >= 4 {
                    unsafe {
                        ptr::write_volatile(self.data.add(write_idx) as *mut u32, PADDING_SENTINEL);
                    }
                }

                let header_ptr = unsafe { self.data.add(offset) as *mut SlotHeader };
                unsafe {
                    (*header_ptr)
                        .owner_pid
                        .store(std::process::id(), Ordering::Release);
                }

                return Some(WriteSlot {
                    ring: self,
                    offset,
                    epoch_claim: write + wrap_padding as u64,
                });
            }
        }
    }

    pub fn try_read_raw(&self) -> Result<Option<RawShmMessage>, CellError> {
        let read = unsafe { (*self.control).read_pos.load(Ordering::Acquire) };
        let write = unsafe { (*self.control).write_pos.load(Ordering::Acquire) };

        if read == write {
            return Ok(None);
        }

        let read_idx = (read % self.capacity as u64) as usize;

        let first_word_ptr = unsafe { self.data.add(read_idx) as *const u32 };
        let first_word = unsafe { ptr::read_volatile(first_word_ptr) };

        let (data_offset, total_consumed) = if first_word == PADDING_SENTINEL {
            let bytes_to_end = self.capacity - read_idx;
            (HEADER_SIZE, bytes_to_end + HEADER_SIZE)
        } else {
            (read_idx + HEADER_SIZE, HEADER_SIZE)
        };

        let header_ptr = unsafe { self.data.add(data_offset - HEADER_SIZE) as *mut SlotHeader };
        let header = unsafe { &mut *header_ptr };

        let expected_epoch = if first_word == PADDING_SENTINEL {
            read + (self.capacity - read_idx) as u64
        } else {
            read
        };

        let gen_before = header.generation.load(Ordering::Acquire);
        if header.epoch.load(Ordering::Acquire) != expected_epoch {
            return Ok(None);
        }

        let data_len = header.len.load(Ordering::Acquire);
        if data_len == 0 {
            return Ok(None);
        }

        let mut rc = header.refcount.load(Ordering::Acquire);

        if rc > 0 {
            let pid = header.owner_pid.load(Ordering::Acquire);
            if pid > 0 && !is_process_alive(pid) {
                header.refcount.store(0, Ordering::Release);
                rc = 0;
            }
        }

        loop {
            if rc != 0 {
                return Ok(None);
            }
            match header
                .refcount
                .compare_exchange_weak(0, 1, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => break,
                Err(curr) => rc = curr,
            }
        }

        let gen_after = header.generation.load(Ordering::Acquire);
        if gen_before != gen_after {
            header.refcount.store(0, Ordering::Release);
            return Err(CellError::Corruption);
        }

        let channel = header.channel.load(Ordering::Acquire);
        let data_ptr = unsafe { self.data.add(data_offset) };

        let slice = unsafe { std::slice::from_raw_parts(data_ptr, data_len as usize) };
        let aligned_len = (data_len as usize + ALIGNMENT - 1) & !(ALIGNMENT - 1);
        let actual_consumed = total_consumed + aligned_len;

        let token = Arc::new(SlotToken {
            ring: self,
            total_consumed: actual_consumed,
        });

        let static_slice: &'static [u8] = unsafe { std::mem::transmute(slice) };

        Ok(Some(RawShmMessage {
            data: static_slice,
            channel,
            _token: token,
        }))
    }

    pub async fn wait_for_slot(&self, size: usize) -> WriteSlot<'_> {
        let mut spin = 0u32;
        loop {
            if let Some(slot) = self.try_alloc(size) {
                return slot;
            }
            spin += 1;
            if spin < 10000 {
                std::hint::spin_loop();
            } else {
                #[cfg(feature = "std")]
                tokio::time::sleep(tokio::time::Duration::from_nanos(100)).await;
                spin = 0;
            }
        }
    }
}

pub struct WriteSlot<'a> {
    ring: &'a RingBuffer,
    offset: usize,
    epoch_claim: u64,
}

impl<'a> WriteSlot<'a> {
    pub fn write(&mut self, data: &[u8], channel: u8) {
        unsafe {
            let dest = self.ring.data.add(self.offset + HEADER_SIZE);
            ptr::copy_nonoverlapping(data.as_ptr(), dest, data.len());

            let header_ptr = self.ring.data.add(self.offset) as *mut SlotHeader;
            (*header_ptr).channel.store(channel, Ordering::Release);
        }
    }

    pub fn commit(self, actual_size: usize) {
        unsafe {
            let header_ptr = self.ring.data.add(self.offset) as *mut SlotHeader;
            (*header_ptr).refcount.store(0, Ordering::Release);
            (*header_ptr)
                .len
                .store(actual_size as u32, Ordering::Release);
            std::sync::atomic::compiler_fence(Ordering::Release);
            (*header_ptr).generation.fetch_add(1, Ordering::Release);
            (*header_ptr)
                .epoch
                .store(self.epoch_claim, Ordering::Release);
        }
    }
}

impl<'a> Drop for WriteSlot<'a> {
    fn drop(&mut self) {}
}

pub struct SlotToken {
    ring: *const RingBuffer,
    total_consumed: usize,
}

unsafe impl Send for SlotToken {}
unsafe impl Sync for SlotToken {}

impl Drop for SlotToken {
    fn drop(&mut self) {
        let ring = unsafe { &*self.ring };
        unsafe {
            (*ring.control)
                .read_pos
                .fetch_add(self.total_consumed as u64, Ordering::Release);
        }
    }
}

pub struct RawShmMessage {
    data: &'static [u8],
    channel: u8,
    _token: Arc<SlotToken>,
}
impl RawShmMessage {
    pub fn get_bytes(&self) -> &'static [u8] {
        self.data
    }
    pub fn channel(&self) -> u8 {
        self.channel
    }
    pub fn token(&self) -> Arc<SlotToken> {
        self._token.clone()
    }
}

pub struct ShmMessage<T: Archive>
where
    <T as Archive>::Archived: 'static,
{
    archived: &'static T::Archived,
    _token: Arc<SlotToken>,
    _phantom: PhantomData<T>,
}

impl<T: Archive> ShmMessage<T>
where
    <T as Archive>::Archived: 'static,
{
    pub fn get(&self) -> &T::Archived {
        self.archived
    }

    pub fn deserialize(&self) -> Result<T, CellError>
    where
        T::Archived: Deserialize<T, rkyv::de::deserializers::SharedDeserializeMap>,
    {
        let mut deserializer = rkyv::de::deserializers::SharedDeserializeMap::new();
        self.archived
            .deserialize(&mut deserializer)
            .map_err(|_| CellError::SerializationFailure)
    }
}

unsafe impl<T: Archive + Send> Send for ShmMessage<T> where <T as Archive>::Archived: 'static {}
unsafe impl<T: Archive + Sync> Sync for ShmMessage<T> where <T as Archive>::Archived: 'static {}

impl<T: Archive> Clone for ShmMessage<T>
where
    <T as Archive>::Archived: 'static,
{
    fn clone(&self) -> Self {
        Self {
            archived: self.archived,
            _token: self._token.clone(),
            _phantom: PhantomData,
        }
    }
}

#[derive(Clone)]
pub struct ShmClient {
    pub tx: Arc<RingBuffer>,
    pub rx: Arc<RingBuffer>,
}

impl ShmClient {
    pub fn new(tx: Arc<RingBuffer>, rx: Arc<RingBuffer>) -> Self {
        Self { tx, rx }
    }

    pub async fn request_raw(
        &self,
        req_bytes: &[u8],
        channel: u8,
    ) -> Result<RawShmMessage, CellError> {
        let size = req_bytes.len();
        let mut slot = self.tx.wait_for_slot(size).await;
        slot.write(req_bytes, channel);
        slot.commit(size);

        let mut spin = 0u32;
        loop {
            match self.rx.try_read_raw() {
                Ok(Some(msg)) => return Ok(msg),
                Ok(None) => {}
                Err(CellError::Corruption) => {
                    // Stale/Corrupt read, retry
                }
                Err(e) => return Err(e),
            }

            spin += 1;
            if spin < 10000 {
                std::hint::spin_loop();
            } else {
                #[cfg(feature = "std")]
                tokio::time::sleep(tokio::time::Duration::from_nanos(100)).await;
                spin = 0;
            }
        }
    }
}

fn is_process_alive(pid: u32) -> bool {
    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    {
        use nix::sys::signal::kill;
        use nix::unistd::Pid;
        match kill(Pid::from_raw(pid as i32), None) {
            Ok(_) => true,
            Err(nix::errno::Errno::ESRCH) => false,
            Err(_) => true,
        }
    }
    #[cfg(not(all(unix, any(target_os = "linux", target_os = "macos"))))]
    {
        let _ = pid;
        true
    }
}
