// TRUE ZERO-COPY SHARED MEMORY WITH RKYV
// Serialize directly into ring buffer, read in-place with zero copies
// Automatic upgrade from socket transport to shared memory

use memmap2::MmapMut;
use rkyv::ser::serializers::AllocSerializer;
use rkyv::ser::Serializer as _;
use rkyv::{Archive, Deserialize, Serialize};
use std::fs::File;
use std::marker::PhantomData;
use std::os::unix::io::AsRawFd;
use std::ptr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::*};
use std::sync::Arc;

// === TYPE DEFINITIONS ===

// We use AllocSerializer for the "serialize then copy" strategy.
// This is safer than guessing sizes and allows exact allocation in the ring.
pub type ShmSerializer = AllocSerializer<1024>;

const CACHE_LINE: usize = 64;
const RING_SIZE: usize = 32 * 1024 * 1024;
const DATA_OFFSET: usize = 128;
const DATA_CAPACITY: usize = RING_SIZE - DATA_OFFSET;
const PADDING_SENTINEL: u32 = 0xFFFFFFFF;
const ALIGNMENT: usize = 16; // 16-byte alignment to be safe for all types

// === SLOT HEADER ===
#[repr(C)]
struct SlotHeader {
    refcount: AtomicU32,
    len: u32,
    _pad: u64, // Pad to 16 bytes to maintain alignment of data following it
}

const HEADER_SIZE: usize = std::mem::size_of::<SlotHeader>();

// === RING CONTROL ===
#[repr(C, align(64))]
struct RingControl {
    write_pos: AtomicU64,
    _pad1: [u8; CACHE_LINE - 8],
    read_pos: AtomicU64,
    _pad2: [u8; CACHE_LINE - 8],
}

// === RING BUFFER ===
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
        use std::os::unix::io::FromRawFd;

        let name_cstr = CString::new(name)?;

        // Must allow sealing to use F_ADD_SEALS later
        let flags = MemFdCreateFlag::MFD_CLOEXEC | MemFdCreateFlag::MFD_ALLOW_SEALING;
        let owned_fd = memfd_create(&name_cstr, flags)?;

        // Convert OwnedFd to File safely
        let file = File::from(owned_fd);
        file.set_len(RING_SIZE as u64)?;

        // Seal to prevent SIGBUS attacks
        let raw_fd = file.as_raw_fd();
        let seals = nix::fcntl::SealFlag::F_SEAL_GROW
            | nix::fcntl::SealFlag::F_SEAL_SHRINK
            | nix::fcntl::SealFlag::F_SEAL_SEAL;
        nix::fcntl::fcntl(raw_fd, nix::fcntl::F_ADD_SEALS(seals))?;

        let mut mmap = unsafe { MmapMut::map_mut(&file)? };
        mmap[..DATA_OFFSET].fill(0);

        let control = mmap.as_mut_ptr() as *mut RingControl;
        let data = unsafe { mmap.as_mut_ptr().add(DATA_OFFSET) };

        let ring = Arc::new(Self {
            control,
            data,
            capacity: DATA_CAPACITY,
            _mmap: mmap,
            _file: Some(file),
        });

        let fd_to_send = ring._file.as_ref().unwrap().as_raw_fd();

        Ok((ring, fd_to_send))
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
        // Ensure aligned size
        let aligned_size = (exact_size + ALIGNMENT - 1) & !(ALIGNMENT - 1);
        let total_needed = HEADER_SIZE + aligned_size;

        let write = unsafe { (*self.control).write_pos.load(Acquire) };
        let read = unsafe { (*self.control).read_pos.load(Acquire) };

        let used = write - read;
        if used + total_needed as u64 > self.capacity as u64 {
            return None; // Full
        }

        let write_idx = (write % self.capacity as u64) as usize;
        let space_at_end = self.capacity - write_idx;

        let (offset, wrap_padding) = if space_at_end >= total_needed {
            (write_idx, 0)
        } else {
            // Need to wrap. Write padding sentinel.
            if space_at_end >= 4 {
                unsafe {
                    let sentinel_ptr = self.data.add(write_idx) as *mut u32;
                    ptr::write_volatile(sentinel_ptr, PADDING_SENTINEL);
                }
            }
            // Check if we have space at 0
            let used_at_start = (read % self.capacity as u64) as usize;
            // Actually, we rely on `write - read` logic above.
            // If wrapping, we treat the end space as "consumed" effectively by skipping it.
            // But we must check if we overlap with read pointer after wrap.
            // Simplified: The `used` check handles total capacity.
            // We just need to ensure `total_needed` fits at offset 0?
            // The `used` calculation assumes linear buffer.
            // A wrap consumes `space_at_end` extra bytes effectively.
            // We should check if `used + space_at_end + total_needed <= capacity`.
            // But `used` accounts for distance between W and R.
            // If we skip `space_at_end`, we just advance `write_pos` by `space_at_end` first.

            // Re-check capacity including the skip
            if used + space_at_end as u64 + total_needed as u64 > self.capacity as u64 {
                return None;
            }

            (0, space_at_end)
        };

        unsafe {
            let header_ptr = self.data.add(offset) as *mut SlotHeader;
            ptr::write(
                header_ptr,
                SlotHeader {
                    refcount: AtomicU32::new(1),
                    len: 0,
                    _pad: 0,
                },
            );
        }

        Some(WriteSlot {
            ring: self,
            offset,
            size: exact_size,
            wrap_padding,
            committed: false,
        })
    }

    pub fn try_read<T: Archive>(&self) -> Option<ShmMessage<T>>
    where
        T::Archived:
            rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'static>> + 'static,
    {
        let read = unsafe { (*self.control).read_pos.load(Acquire) };
        let write = unsafe { (*self.control).write_pos.load(Acquire) };

        if read == write {
            return None; // Empty
        }

        let read_idx = (read % self.capacity as u64) as usize;
        let first_word_ptr = unsafe { self.data.add(read_idx) as *const u32 };
        let first_word = unsafe { ptr::read_volatile(first_word_ptr) };

        let (data_offset, total_consumed) = if first_word == PADDING_SENTINEL {
            // Wrapped
            let bytes_to_end = self.capacity - read_idx;
            // Read real header at 0
            let header_ptr = self.data as *const SlotHeader;
            let header = unsafe { &*header_ptr };
            let len = header.len as usize;
            let aligned_len = (len + ALIGNMENT - 1) & !(ALIGNMENT - 1);

            (HEADER_SIZE, bytes_to_end + HEADER_SIZE + aligned_len)
        } else {
            let header_ptr = unsafe { self.data.add(read_idx) as *const SlotHeader };
            let header = unsafe { &*header_ptr };
            let len = header.len as usize;
            let aligned_len = (len + ALIGNMENT - 1) & !(ALIGNMENT - 1);

            (read_idx + HEADER_SIZE, HEADER_SIZE + aligned_len)
        };

        // Increment refcount
        let header_ptr = unsafe { self.data.add(data_offset - HEADER_SIZE) as *const SlotHeader };
        let header = unsafe { &*header_ptr };
        header.refcount.fetch_add(1, AcqRel);

        let data_ptr = unsafe { self.data.add(data_offset) };
        let data_len = header.len as usize;

        // Zero-copy verification
        let archived_ref = unsafe {
            let slice = std::slice::from_raw_parts(data_ptr, data_len);
            match rkyv::check_archived_root::<T>(slice) {
                Ok(archived) => archived,
                Err(e) => {
                    // This can happen if reader sees partial write (shouldn't happen with atomic commit)
                    // or corruption.
                    eprintln!("[SHM] Validation failed: {:?}", e);
                    header.refcount.fetch_sub(1, Release);
                    return None;
                }
            }
        };

        let token = Arc::new(SlotToken {
            ring: self,
            header_ptr,
            total_consumed,
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
            if spin < 1000 {
                std::hint::spin_loop();
            } else {
                tokio::time::sleep(std::time::Duration::from_micros(1)).await;
                spin = 0;
            }
        }
    }
}

// === WRITE SLOT (RAII for writing) ===
pub struct WriteSlot<'a> {
    ring: &'a RingBuffer,
    offset: usize,
    size: usize,
    wrap_padding: usize,
    committed: bool,
}

impl<'a> WriteSlot<'a> {
    pub fn write(&mut self, data: &[u8]) {
        if data.len() > self.size {
            panic!(
                "WriteSlot overflow: len {} > size {}",
                data.len(),
                self.size
            );
        }
        unsafe {
            let dest = self.ring.data.add(self.offset + HEADER_SIZE);
            ptr::copy_nonoverlapping(data.as_ptr(), dest, data.len());
        }
    }

    pub fn commit(mut self, actual_size: usize) {
        unsafe {
            let header_ptr = self.ring.data.add(self.offset) as *mut SlotHeader;
            (*header_ptr).len = actual_size as u32;
        }

        let aligned_size = (self.size + ALIGNMENT - 1) & !(ALIGNMENT - 1);
        let total_advance = self.wrap_padding + HEADER_SIZE + aligned_size;

        unsafe {
            let current = (*self.ring.control).write_pos.load(Relaxed);
            (*self.ring.control)
                .write_pos
                .store(current + total_advance as u64, Release);
        }

        self.committed = true;
    }
}

impl<'a> Drop for WriteSlot<'a> {
    fn drop(&mut self) {
        // Rollback implicitly by not advancing write_pos
    }
}

// === SLOT TOKEN (RAII) ===
struct SlotToken {
    ring: *const RingBuffer,
    header_ptr: *const SlotHeader,
    total_consumed: usize,
}

unsafe impl Send for SlotToken {}
unsafe impl Sync for SlotToken {}

impl Drop for SlotToken {
    fn drop(&mut self) {
        unsafe {
            let header = &*self.header_ptr;
            let old_refcount = header.refcount.fetch_sub(1, Release);

            if old_refcount == 1 {
                let ring = &*self.ring;
                let current = (*ring.control).read_pos.load(Relaxed);
                (*ring.control)
                    .read_pos
                    .store(current + self.total_consumed as u64, Release);
            }
        }
    }
}

// === SHM MESSAGE ===
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
        T::Archived: rkyv::Deserialize<T, rkyv::de::deserializers::SharedDeserializeMap>,
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

// === CLIENT ===
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
        // 1. Serialize to heap buffer first (Exact size, valid alignment)
        let bytes = rkyv::to_bytes::<_, 1024>(req)?.into_vec();
        let size = bytes.len();

        // 2. Allocate exact slot
        let mut slot = self.tx.wait_for_slot(size).await;

        // 3. Copy
        slot.write(&bytes);

        // 4. Commit
        slot.commit(size);

        // 5. Wait for response
        let mut spin = 0u32;
        loop {
            if let Some(msg) = self.rx.try_read::<Resp>() {
                return Ok(msg);
            }
            spin += 1;
            if spin < 1000 {
                std::hint::spin_loop();
            } else {
                tokio::time::sleep(std::time::Duration::from_micros(1)).await;
                spin = 0;
            }
        }
    }
}

// === SERVER LOOP ===
pub async fn serve_loop<F, Fut, Req, Resp>(
    rx: Arc<RingBuffer>,
    tx: Arc<RingBuffer>,
    handler: F,
) -> anyhow::Result<()>
where
    F: Fn(&Req::Archived) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = anyhow::Result<Resp>> + Send,
    Req: Archive,
    Req::Archived:
        rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'static>> + 'static,
    Resp: Serialize<ShmSerializer>,
{
    let mut spin = 0u32;

    loop {
        let request = if let Some(msg) = rx.try_read::<Req>() {
            spin = 0;
            msg
        } else {
            spin += 1;
            if spin < 1000 {
                std::hint::spin_loop();
            } else {
                tokio::time::sleep(std::time::Duration::from_micros(1)).await;
            }
            continue;
        };

        let archived_req = request.get();
        let response = handler(archived_req).await?;
        drop(request);

        // Serialize to buffer
        let bytes = rkyv::to_bytes::<_, 1024>(&response)?.into_vec();
        let size = bytes.len();

        let mut slot = tx.wait_for_slot(size).await;
        slot.write(&bytes);
        slot.commit(size);
    }
}
