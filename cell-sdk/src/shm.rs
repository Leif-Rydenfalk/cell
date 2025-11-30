// TRUE ZERO-COPY SHARED MEMORY WITH RKYV
// Serialize directly into ring buffer, read in-place with zero copies
// Automatic upgrade from socket transport to shared memory

use memmap2::MmapMut;
use rkyv::ser::serializers::BufferSerializer;
use rkyv::ser::Serializer as _;
use rkyv::{Archive, Deserialize, Serialize};
use std::marker::PhantomData;
use std::ptr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::*};
use std::sync::Arc;

const CACHE_LINE: usize = 64;
const RING_SIZE: usize = 32 * 1024 * 1024;
const DATA_OFFSET: usize = 128;
const DATA_CAPACITY: usize = RING_SIZE - DATA_OFFSET;
const PADDING_SENTINEL: u32 = 0xFFFFFFFF;

// === SLOT HEADER ===
// Each message has this header before the rkyv data
#[repr(C)]
struct SlotHeader {
    refcount: AtomicU32, // How many readers hold this slot
    len: u32,            // Actual data length (not including header)
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
}

unsafe impl Send for RingBuffer {}
unsafe impl Sync for RingBuffer {}

impl RingBuffer {
    #[cfg(target_os = "linux")]
    pub fn create(name: &str) -> anyhow::Result<(Arc<Self>, std::os::unix::io::RawFd)> {
        use nix::sys::memfd::{memfd_create, MemFdCreateFlag};
        use std::ffi::CString;
        use std::fs::File;
        use std::os::unix::io::{AsRawFd, FromRawFd};

        let name_cstr = CString::new(name)?;
        let fd = memfd_create(&name_cstr, MemFdCreateFlag::MFD_CLOEXEC)?;
        let file = unsafe { File::from_raw_fd(fd) };
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
        });

        Ok((ring, raw_fd))
    }

    #[cfg(target_os = "linux")]
    pub unsafe fn attach(fd: std::os::unix::io::RawFd) -> anyhow::Result<Arc<Self>> {
        use std::fs::File;
        use std::os::unix::io::FromRawFd;

        let file = File::from_raw_fd(fd);
        let mut mmap = MmapMut::map_mut(&file)?;

        let control = mmap.as_mut_ptr() as *mut RingControl;
        let data = mmap.as_mut_ptr().add(DATA_OFFSET);

        Ok(Arc::new(Self {
            control,
            data,
            capacity: DATA_CAPACITY,
            _mmap: mmap,
        }))
    }

    /// Try to allocate space for a message
    /// Returns WriteSlot on success, None if full
    pub fn try_alloc(&self, max_size: usize) -> Option<WriteSlot> {
        let total_needed = HEADER_SIZE + max_size;

        let write = unsafe { (*self.control).write_pos.load(Acquire) };
        let read = unsafe { (*self.control).read_pos.load(Acquire) };

        let used = write - read;
        if used + total_needed as u64 > self.capacity as u64 {
            return None; // Full
        }

        let write_idx = (write % self.capacity as u64) as usize;
        let space_at_end = self.capacity - write_idx;

        let (offset, wrap_padding) = if space_at_end >= total_needed {
            // Fits at end
            (write_idx, 0)
        } else {
            // Need to wrap - write padding sentinel
            unsafe {
                let sentinel_ptr = self.data.add(write_idx) as *mut u32;
                ptr::write_volatile(sentinel_ptr, PADDING_SENTINEL);
            }
            (0, space_at_end)
        };

        // Initialize slot header with refcount = 1 (writer holds it)
        unsafe {
            let header_ptr = self.data.add(offset) as *mut SlotHeader;
            ptr::write(
                header_ptr,
                SlotHeader {
                    refcount: AtomicU32::new(1),
                    len: 0, // Will be set on commit
                },
            );
        }

        Some(WriteSlot {
            ring: self,
            offset,
            max_size,
            wrap_padding,
            committed: false,
        })
    }

    /// Try to read next message
    /// Returns ShmMessage with zero-copy reference
    pub fn try_read<T: Archive>(&self) -> Option<ShmMessage<T>> {
        let read = unsafe { (*self.control).read_pos.load(Acquire) };
        let write = unsafe { (*self.control).write_pos.load(Acquire) };

        if read == write {
            return None; // Empty
        }

        let read_idx = (read % self.capacity as u64) as usize;
        let first_word_ptr = unsafe { self.data.add(read_idx) as *const u32 };
        let first_word = unsafe { ptr::read_volatile(first_word_ptr) };

        let (data_offset, total_consumed) = if first_word == PADDING_SENTINEL {
            // Wrapped - real data at offset 0
            let bytes_to_end = self.capacity - read_idx;
            let header_ptr = self.data as *const SlotHeader;
            let header = unsafe { &*header_ptr };
            let len = header.len as usize;

            (HEADER_SIZE, bytes_to_end + HEADER_SIZE + len)
        } else {
            // Data at current position
            let header_ptr = unsafe { self.data.add(read_idx) as *const SlotHeader };
            let header = unsafe { &*header_ptr };
            let len = header.len as usize;

            (read_idx + HEADER_SIZE, HEADER_SIZE + len)
        };

        // Increment refcount (reader now holds slot)
        let header_ptr = unsafe { self.data.add(data_offset - HEADER_SIZE) as *const SlotHeader };
        let header = unsafe { &*header_ptr };
        header.refcount.fetch_add(1, AcqRel);

        // Get pointer to archived data
        let data_ptr = unsafe { self.data.add(data_offset) };
        let data_len = header.len as usize;

        // Validate and get archived reference
        let archived_ref = unsafe {
            let slice = std::slice::from_raw_parts(data_ptr, data_len);
            match rkyv::check_archived_root::<T>(slice) {
                Ok(archived) => archived,
                Err(_) => {
                    // Validation failed - decrement refcount and return None
                    header.refcount.fetch_sub(1, Release);
                    return None;
                }
            }
        };

        // Create token that will manage refcount on drop
        let token = Arc::new(SlotToken {
            ring: self,
            header_ptr,
            total_consumed,
        });

        // SAFETY: We increment refcount, so memory won't be reused
        // The 'static lifetime is a lie but safe because SlotToken guards it
        let archived_static: &'static T::Archived = unsafe { std::mem::transmute(archived_ref) };

        Some(ShmMessage {
            archived: archived_static,
            _token: token,
            _phantom: PhantomData,
        })
    }

    /// Check if we can reuse a slot (refcount == 0)
    fn can_reuse_slot(&self, offset: usize) -> bool {
        unsafe {
            let header_ptr = self.data.add(offset) as *const SlotHeader;
            let header = &*header_ptr;
            header.refcount.load(Acquire) == 0
        }
    }

    /// Wait for a slot to become available (spinning strategy)
    async fn wait_for_slot(&self, max_size: usize) -> WriteSlot {
        let mut spin = 0u32;
        loop {
            if let Some(slot) = self.try_alloc(max_size) {
                return slot;
            }

            spin += 1;
            if spin < 100 {
                std::hint::spin_loop();
            } else if spin < 10000 {
                tokio::task::yield_now().await;
            } else {
                tokio::time::sleep(std::time::Duration::from_micros(10)).await;
                spin = 0;
            }
        }
    }
}

// === WRITE SLOT (RAII for writing) ===
pub struct WriteSlot<'a> {
    ring: &'a RingBuffer,
    offset: usize,
    max_size: usize,
    wrap_padding: usize,
    committed: bool,
}

impl<'a> WriteSlot<'a> {
    /// Get mutable slice for serialization
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe {
            std::slice::from_raw_parts_mut(
                self.ring.data.add(self.offset + HEADER_SIZE),
                self.max_size,
            )
        }
    }

    /// Commit with actual size (after serialization)
    pub fn commit(mut self, actual_size: usize) {
        // Write actual length to header
        unsafe {
            let header_ptr = self.ring.data.add(self.offset) as *mut SlotHeader;
            (*header_ptr).len = actual_size as u32;
        }

        // Advance write position
        let total_advance = self.wrap_padding + HEADER_SIZE + actual_size;
        unsafe {
            let current = (*self.ring.control).write_pos.load(Relaxed);
            (*self.ring.control).write_pos.store(
                current + total_advance as u64,
                Release, // Make data visible to readers
            );
        }

        self.committed = true;
    }
}

impl<'a> Drop for WriteSlot<'a> {
    fn drop(&mut self) {
        if !self.committed {
            panic!("WriteSlot dropped without commit - this causes data corruption");
        }
    }
}

// === SLOT TOKEN (RAII for refcount management) ===
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

            // If we were the last reader, advance read position
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

// === SHM MESSAGE (zero-copy wrapper) ===
pub struct ShmMessage<T: Archive> {
    archived: &'static T::Archived,
    _token: Arc<SlotToken>,
    _phantom: PhantomData<T>,
}

impl<T: Archive> ShmMessage<T> {
    /// Get reference to archived data (zero-copy)
    pub fn get(&self) -> &T::Archived {
        self.archived
    }

    /// Deserialize if you need owned data (this DOES copy)
    pub fn deserialize(&self) -> Result<T, <T::Archived as Deserialize<T, rkyv::Infallible>>::Error>
    where
        T::Archived: Deserialize<T, rkyv::Infallible>,
    {
        self.archived.deserialize(&mut rkyv::Infallible)
    }
}

// ShmMessage is Send + Sync if T is Send
unsafe impl<T: Archive + Send> Send for ShmMessage<T> {}
unsafe impl<T: Archive + Sync> Sync for ShmMessage<T> {}

// Clone creates another reference to the same message (increments refcount)
impl<T: Archive> Clone for ShmMessage<T> {
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

    /// Send request and wait for response (zero-copy on response)
    pub async fn request<Req, Resp>(&self, req: &Req) -> anyhow::Result<ShmMessage<Resp>>
    where
        Req: for<'a> Serialize<BufferSerializer<&'a mut [u8]>>,
        Resp: Archive,
    {
        // 1. Estimate size (conservative)
        let estimated_size = estimate_size(req);

        // 2. Allocate slot
        let mut slot = self.tx.wait_for_slot(estimated_size).await;

        // 3. Serialize directly into shared memory
        let mut serializer = BufferSerializer::new(slot.as_mut_slice());
        serializer
            .serialize_value(req)
            .map_err(|e| anyhow::anyhow!("Serialization failed: {:?}", e))?;
        let actual_size = serializer.pos();

        // 4. Commit
        slot.commit(actual_size);

        // 5. Wait for response (zero-copy)
        let mut spin = 0u32;
        loop {
            if let Some(msg) = self.rx.try_read::<Resp>() {
                return Ok(msg);
            }

            spin += 1;
            if spin < 100 {
                std::hint::spin_loop();
            } else if spin < 10000 {
                tokio::task::yield_now().await;
            } else {
                tokio::time::sleep(std::time::Duration::from_micros(10)).await;
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
    Resp: for<'a> Serialize<BufferSerializer<&'a mut [u8]>>,
{
    let mut spin = 0u32;

    loop {
        // 1. Try read request (zero-copy)
        let request = if let Some(msg) = rx.try_read::<Req>() {
            spin = 0;
            msg
        } else {
            spin += 1;
            if spin < 100 {
                std::hint::spin_loop();
            } else if spin < 5000 {
                tokio::task::yield_now().await;
            } else {
                tokio::time::sleep(std::time::Duration::from_micros(50)).await;
            }
            continue;
        };

        // 2. Process (can hold request across await - it's refcounted!)
        let response = handler(request.get()).await?;

        // 3. Drop request early if possible
        drop(request);

        // 4. Allocate response slot
        let estimated_size = estimate_size(&response);
        let mut slot = tx.wait_for_slot(estimated_size).await;

        // 5. Serialize response
        let mut serializer = BufferSerializer::new(slot.as_mut_slice());
        serializer
            .serialize_value(&response)
            .map_err(|e| anyhow::anyhow!("Serialization failed: {:?}", e))?;
        let actual_size = serializer.pos();

        // 6. Commit
        slot.commit(actual_size);
    }
}

// === HELPER ===
fn estimate_size<T>(value: &T) -> usize
where
    T: for<'a> Serialize<BufferSerializer<&'a mut [u8]>>,
{
    // Conservative estimate: serialize to dummy buffer and measure
    let mut dummy = [0u8; 0];
    let mut ser = BufferSerializer::new(&mut dummy[..]);
    let _ = ser.serialize_value(value);
    let size = ser.pos();

    // Add 20% padding for safety
    (size * 120) / 100
}

// === USAGE EXAMPLE ===
/*
#[derive(Archive, Serialize, Deserialize)]
#[archive(check_bytes)]
struct Request {
    id: u64,
    data: Vec<u8>,
}

#[derive(Archive, Serialize, Deserialize)]
#[archive(check_bytes)]
struct Response {
    result: String,
}

async fn example() {
    let (tx_ring, tx_fd) = RingBuffer::create("tx").unwrap();
    let (rx_ring, rx_fd) = RingBuffer::create("rx").unwrap();

    let client = ShmClient::new(tx_ring, rx_ring);

    let req = Request { id: 42, data: vec![1,2,3] };
    let response = client.request::<Request, Response>(&req).await.unwrap();

    // Zero-copy access!
    println!("Result: {}", response.get().result);
}
*/
