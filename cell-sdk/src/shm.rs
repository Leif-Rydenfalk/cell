use memmap2::MmapMut;
use rkyv::ser::serializers::AllocSerializer;
use rkyv::ser::Serializer as _;
use rkyv::{Archive, Deserialize, Serialize};
use std::fs::File;
use std::marker::PhantomData;
use std::os::unix::io::AsRawFd;
use std::ptr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
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
    // NEW: Epoch ensures we strictly read data for the current position
    epoch: AtomicU64,
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

        let name_cstr = CString::new(name)?;
        let flags = MemFdCreateFlag::MFD_CLOEXEC | MemFdCreateFlag::MFD_ALLOW_SEALING;
        let owned_fd = memfd_create(&name_cstr, flags)?;
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

    #[cfg(target_os = "linux")]
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

    pub fn try_alloc(&self, exact_size: usize) -> Option<WriteSlot> {
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

                // We do NOT zero the header here. We overwrite it in commit().
                // Zeroing here would race with the reader checking epoch.

                return Some(WriteSlot {
                    ring: self,
                    offset,
                    // Pass the exact write position we claimed.
                    // This is our "Epoch" or "Sequence Number".
                    epoch_claim: write + wrap_padding as u64,
                });
            }
        }
    }

    pub fn try_read<T: Archive>(&self) -> Option<ShmMessage<T>>
    where
        T::Archived:
            rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'static>> + 'static,
    {
        let read = unsafe { (*self.control).read_pos.load(Ordering::Acquire) };
        let write = unsafe { (*self.control).write_pos.load(Ordering::Acquire) };

        if read == write {
            return None;
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

        let header_ptr = unsafe { self.data.add(data_offset - HEADER_SIZE) as *const SlotHeader };
        let header = unsafe { &*header_ptr };

        // 1. EPOCH CHECK (The Fix)
        // We calculate what the write_pos SHOULD be for this data to be valid.
        // It should match the current monotonic read_pos (adjusted for wrapping if we consumed padding).
        // Wait, 'read' variable is the monotonic counter.
        // If we hit sentinel, we consumed bytes_to_end. The REAL data starts at `read + bytes_to_end`.

        let expected_epoch = if first_word == PADDING_SENTINEL {
            read + (self.capacity - read_idx) as u64
        } else {
            read
        };

        let found_epoch = header.epoch.load(Ordering::Acquire);

        if found_epoch != expected_epoch {
            // Stale data. Writer has reserved space but hasn't committed yet.
            return None;
        }

        // 2. Data is valid, proceed
        let data_len = header.len.load(Ordering::Acquire);
        if data_len == 0 {
            return None;
        } // Should not happen if epoch matches, but safe.

        // 3. Claim Refcount
        let mut rc = header.refcount.load(Ordering::Acquire);
        loop {
            if rc != 0 {
                return None;
            } // Taken by another thread
            match header
                .refcount
                .compare_exchange_weak(0, 1, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => break,
                Err(curr) => rc = curr,
            }
        }

        let aligned_len = (data_len as usize + ALIGNMENT - 1) & !(ALIGNMENT - 1);
        let actual_consumed = if first_word == PADDING_SENTINEL {
            total_consumed + aligned_len
        } else {
            total_consumed + aligned_len
        };

        // 4. Validate and Return
        let data_ptr = unsafe { self.data.add(data_offset) };
        let archived_ref = unsafe {
            let slice = std::slice::from_raw_parts(data_ptr, data_len as usize);
            match rkyv::check_archived_root::<T>(slice) {
                Ok(a) => a,
                Err(_) => {
                    // Corruption/Panic recovery
                    header.refcount.store(0, Ordering::Release);
                    return None;
                }
            }
        };

        let token = Arc::new(SlotToken {
            ring: self,
            total_consumed: actual_consumed,
        });

        let archived_static: &'static T::Archived = unsafe { std::mem::transmute(archived_ref) };

        Some(ShmMessage {
            archived: archived_static,
            _token: token,
            _phantom: PhantomData,
        })
    }

    pub async fn wait_for_slot(&self, size: usize) -> WriteSlot {
        let mut spin = 0u32;
        loop {
            if let Some(slot) = self.try_alloc(size) {
                return slot;
            }
            spin += 1;
            if spin < 10000 {
                std::hint::spin_loop();
            } else {
                tokio::time::sleep(std::time::Duration::from_nanos(100)).await;
                spin = 0;
            }
        }
    }
}

pub struct WriteSlot<'a> {
    ring: &'a RingBuffer,
    offset: usize,
    epoch_claim: u64, // The epoch we must write to validate this slot
}

impl<'a> WriteSlot<'a> {
    pub fn write(&mut self, data: &[u8]) {
        unsafe {
            let dest = self.ring.data.add(self.offset + HEADER_SIZE);
            ptr::copy_nonoverlapping(data.as_ptr(), dest, data.len());
        }
    }

    pub fn commit(self, actual_size: usize) {
        unsafe {
            let header_ptr = self.ring.data.add(self.offset) as *mut SlotHeader;

            // 1. Initialize refcount (fresh)
            (*header_ptr).refcount.store(0, Ordering::Relaxed);

            // 2. Ensure data writes are visible
            std::sync::atomic::fence(Ordering::Release);

            // 3. Write length
            (*header_ptr)
                .len
                .store(actual_size as u32, Ordering::Relaxed);

            // 4. COMMIT: Write Epoch. This makes the slot valid for the Reader.
            (*header_ptr)
                .epoch
                .store(self.epoch_claim, Ordering::Release);
        }
    }
}

impl<'a> Drop for WriteSlot<'a> {
    fn drop(&mut self) {}
}

struct SlotToken {
    ring: *const RingBuffer,
    total_consumed: usize,
}

unsafe impl Send for SlotToken {}
unsafe impl Sync for SlotToken {}

impl Drop for SlotToken {
    fn drop(&mut self) {
        let ring = unsafe { &*self.ring };
        // Advance read_pos to free space
        unsafe {
            (*ring.control)
                .read_pos
                .fetch_add(self.total_consumed as u64, Ordering::Release);
        }
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

    pub fn deserialize(&self) -> Result<T, rkyv::de::deserializers::SharedDeserializeMapError>
    where
        T::Archived: Deserialize<T, rkyv::de::deserializers::SharedDeserializeMap>,
    {
        let mut deserializer = rkyv::de::deserializers::SharedDeserializeMap::new();
        self.archived.deserialize(&mut deserializer)
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

pub struct ShmClient {
    tx: Arc<RingBuffer>,
    rx: Arc<RingBuffer>,
}

impl ShmClient {
    pub fn new(tx: Arc<RingBuffer>, rx: Arc<RingBuffer>) -> Self {
        Self { tx, rx }
    }

    pub async fn request<Req, Resp>(&self, req: &Req) -> anyhow::Result<ShmMessage<Resp>>
    where
        Req: Serialize<ShmSerializer>,
        Resp: Archive,
        Resp::Archived:
            rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'static>> + 'static,
    {
        let bytes = rkyv::to_bytes::<_, 1024>(req)?.into_vec();
        let size = bytes.len();

        let mut slot = self.tx.wait_for_slot(size).await;
        slot.write(&bytes);
        slot.commit(size);

        let mut spin = 0u32;
        loop {
            if let Some(msg) = self.rx.try_read::<Resp>() {
                return Ok(msg);
            }
            spin += 1;
            if spin < 10000 {
                std::hint::spin_loop();
            } else {
                tokio::time::sleep(std::time::Duration::from_nanos(100)).await;
                spin = 0;
            }
        }
    }
}
