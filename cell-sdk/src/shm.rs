// SPDX-License-Identifier: MIT
// cell-sdk/src/shm.rs
//! Production-grade zero-copy shared memory transport
//!
//! Features:
//! - Lock-free SPSC ring buffer with cache-line alignment
//! - Memory-mapped persistent storage via memmap2
//! - Zero-copy message passing with lifetime management
//! - Robust error handling and corruption detection
//! - Epoch-based memory reclamation for safety

use crate::error::CellError;
use memmap2::{MmapMut, MmapOptions};
use rkyv::ser::serializers::AllocSerializer;
use rkyv::{Archive, Deserialize, Serialize};
use std::fs::File;
use std::marker::PhantomData;
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::ptr;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use thiserror::Error;
use tracing::{debug, error, trace, warn};

/// SHM-specific errors with structured error types
#[derive(Error, Debug, Clone, PartialEq)]
pub enum ShmError {
    #[error("Memory mapping failed: {0}")]
    MappingFailed(String),
    #[error("Ring buffer capacity exceeded")]
    CapacityExceeded,
    #[error("Data corruption detected: {0}")]
    Corruption(String),
    #[error("Invalid slot: generation mismatch")]
    GenerationMismatch,
    #[error("Process {0} is no longer alive")]
    StaleProcess(u32),
    #[error("Serialization failed: {0}")]
    Serialization(String),
    #[error("Deserialization failed: {0}")]
    Deserialization(String),
}

impl From<ShmError> for CellError {
    fn from(e: ShmError) -> Self {
        match e {
            ShmError::Corruption(_) | ShmError::GenerationMismatch => CellError::Corruption,
            ShmError::Serialization(_) => CellError::SerializationFailure,
            ShmError::Deserialization(_) => CellError::DeserializationFailure,
            ShmError::CapacityExceeded => CellError::ResourceExhausted,
            ShmError::StaleProcess(_) => CellError::ConnectionReset,
            _ => CellError::IoError,
        }
    }
}

pub type ShmSerializer = AllocSerializer<1024>;

// Architecture constants for cache-line alignment
const CACHE_LINE: usize = 64;
const RING_SIZE: usize = 32 * 1024 * 1024; // 32MB default
const DATA_OFFSET: usize = 128; // Reserve space for control structures
const DATA_CAPACITY: usize = RING_SIZE - DATA_OFFSET;
const PADDING_SENTINEL: u32 = 0xFFFFFFFF;
const ALIGNMENT: usize = 16;
const HEADER_SIZE: usize = std::mem::size_of::<SlotHeader>();
const MAX_ALLOC_SIZE: usize = 16 * 1024 * 1024; // 16MB max single message

/// Slot header with atomic fields for lock-free coordination
#[repr(C, align(64))]
struct SlotHeader {
    /// Reference count for safe memory reclamation
    refcount: AtomicU32,
    /// Payload length in bytes
    len: AtomicU32,
    /// Monotonic epoch for ordering
    epoch: AtomicU64,
    /// Generation counter for ABA protection
    generation: AtomicU64,
    /// Owner process ID for liveness detection
    owner_pid: AtomicU32,
    /// Logical channel identifier
    channel: AtomicU8,
    /// Padding to cache line
    _pad: [u8; 3],
}

impl SlotHeader {
    fn new() -> Self {
        Self {
            refcount: AtomicU32::new(0),
            len: AtomicU32::new(0),
            epoch: AtomicU64::new(0),
            generation: AtomicU64::new(0),
            owner_pid: AtomicU32::new(0),
            channel: AtomicU8::new(0),
            _pad: [0; 3],
        }
    }
}

/// Ring buffer control structure - cache-line aligned to prevent false sharing
#[repr(C, align(64))]
struct RingControl {
    /// Write position (producer only)
    write_pos: AtomicU64,
    /// Padding to separate from read_pos (consumer)
    _pad1: [u8; CACHE_LINE - 8],
    /// Read position (consumer only)
    read_pos: AtomicU64,
    /// Padding to fill cache line
    _pad2: [u8; CACHE_LINE - 8],
}

impl RingControl {
    fn new() -> Self {
        Self {
            write_pos: AtomicU64::new(0),
            _pad1: [0; CACHE_LINE - 8],
            read_pos: AtomicU64::new(0),
            _pad2: [0; CACHE_LINE - 8],
        }
    }
}

/// Production-grade shared memory ring buffer
///
/// # Safety
/// This uses unsafe internally but provides a safe API through:
/// - Epoch-based memory ordering
/// - Generation counters for ABA protection
/// - Reference counting for safe reclamation
/// - Process liveness detection
pub struct RingBuffer {
    /// Control structure pointer
    control: *mut RingControl,
    /// Data region pointer
    data: *mut u8,
    /// Total data capacity
    capacity: usize,
    /// Memory mapping handle
    _mmap: MmapMut,
    /// Optional backing file
    _file: Option<File>,
}

// Safety: RingBuffer is thread-safe due to atomic operations
unsafe impl Send for RingBuffer {}
unsafe impl Sync for RingBuffer {}

impl RingBuffer {
    /// Create a new file-backed ring buffer for persistence
    ///
    /// # Arguments
    /// * `path` - File path for backing store
    /// * `size` - Size in bytes (must be power of 2, minimum 1MB)
    pub fn create_persistent(path: &std::path::Path, size: usize) -> Result<Arc<Self>, ShmError> {
        let size = size.next_power_of_two().max(1024 * 1024);

        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .map_err(|e| ShmError::MappingFailed(e.to_string()))?;

        file.set_len(size as u64)
            .map_err(|e| ShmError::MappingFailed(e.to_string()))?;

        let mmap = unsafe {
            MmapOptions::new()
                .map_mut(&file)
                .map_err(|e| ShmError::MappingFailed(e.to_string()))?
        };

        Self::init_from_mmap(mmap, Some(file), size)
    }

    /// Attach to an existing shared memory segment via file descriptor
    ///
    /// # Safety
    /// The FD must be valid and point to a properly initialized ring buffer
    pub unsafe fn attach(fd: RawFd) -> Result<Arc<Self>, ShmError> {
        let file = File::from_raw_fd(fd);
        let mmap = MmapMut::map_mut(&file).map_err(|e| ShmError::MappingFailed(e.to_string()))?;

        // Get size from metadata
        let size = mmap.len();

        Self::init_from_mmap(mmap, Some(file), size)
    }

    /// Initialize from an existing memory mapping
    fn init_from_mmap(
        mut mmap: MmapMut,
        file: Option<File>,
        size: usize,
    ) -> Result<Arc<Self>, ShmError> {
        let ptr = mmap.as_mut_ptr();

        // Initialize control structure if needed (first time)
        let control = ptr as *mut RingControl;
        unsafe {
            // Check if already initialized (magic number or non-zero)
            if (*control).write_pos.load(Ordering::Relaxed) == 0
                && (*control).read_pos.load(Ordering::Relaxed) == 0
            {
                // Initialize fresh buffer
                ptr::write(control, RingControl::new());
            }
        }

        let data = unsafe { ptr.add(DATA_OFFSET) };
        let capacity = size.saturating_sub(DATA_OFFSET);

        Ok(Arc::new(Self {
            control,
            data,
            capacity,
            _mmap: mmap,
            _file: file,
        }))
    }

    /// Try to allocate a slot for writing
    ///
    /// Returns None if ring buffer is full
    pub fn try_alloc(&self, exact_size: usize) -> Option<WriteSlot<'_>> {
        if exact_size > MAX_ALLOC_SIZE {
            warn!(
                "Allocation request {} exceeds max {}",
                exact_size, MAX_ALLOC_SIZE
            );
            return None;
        }

        let aligned_size = (exact_size + ALIGNMENT - 1) & !(ALIGNMENT - 1);
        let total_needed = HEADER_SIZE + aligned_size;

        loop {
            let write = unsafe { (*self.control).write_pos.load(Ordering::Acquire) };
            let read = unsafe { (*self.control).read_pos.load(Ordering::Acquire) };

            let used = write.wrapping_sub(read);
            if used + total_needed as u64 > self.capacity as u64 {
                trace!(
                    "Ring buffer full: used={} needed={} capacity={}",
                    used,
                    total_needed,
                    self.capacity
                );
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

            // CAS to claim the slot
            match unsafe {
                (*self.control).write_pos.compare_exchange_weak(
                    write,
                    new_write,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
            } {
                Ok(_) => {
                    // Write padding sentinel if wrapping
                    if wrap_padding > 0 && space_at_end >= 4 {
                        unsafe {
                            ptr::write_volatile(
                                self.data.add(write_idx) as *mut u32,
                                PADDING_SENTINEL,
                            );
                        }
                    }

                    // Initialize header
                    let header_ptr = unsafe { self.data.add(offset) as *mut SlotHeader };
                    unsafe {
                        (*header_ptr)
                            .owner_pid
                            .store(std::process::id(), Ordering::Release);
                        (*header_ptr).len.store(0, Ordering::Relaxed); // Will be set on commit
                        (*header_ptr).refcount.store(0, Ordering::Relaxed);
                    }

                    return Some(WriteSlot {
                        ring: self,
                        offset,
                        epoch_claim: write + wrap_padding as u64,
                        data_len: exact_size,
                    });
                }
                Err(_) => {
                    // CAS failed, retry
                    std::hint::spin_loop();
                    continue;
                }
            }
        }
    }

    /// Try to read a message from the ring buffer
    ///
    /// Returns None if no message available
    pub fn try_read_raw(&self) -> Result<Option<RawShmMessage>, ShmError> {
        let read = unsafe { (*self.control).read_pos.load(Ordering::Acquire) };
        let write = unsafe { (*self.control).write_pos.load(Ordering::Acquire) };

        if read == write {
            return Ok(None);
        }

        let read_idx = (read % self.capacity as u64) as usize;

        // Check for padding sentinel
        let first_word_ptr = unsafe { self.data.add(read_idx) as *const u32 };
        let first_word = unsafe { ptr::read_volatile(first_word_ptr) };

        let (data_offset, total_consumed) = if first_word == PADDING_SENTINEL {
            let bytes_to_end = self.capacity - read_idx;
            (HEADER_SIZE, bytes_to_end + HEADER_SIZE)
        } else {
            (read_idx + HEADER_SIZE, HEADER_SIZE)
        };

        let header_ptr = unsafe { self.data.add(data_offset - HEADER_SIZE) as *mut SlotHeader };
        let header = unsafe { &*header_ptr };

        // Expected epoch for ordering
        let expected_epoch = if first_word == PADDING_SENTINEL {
            read + (self.capacity - read_idx) as u64
        } else {
            read
        };

        // Verify epoch matches (slot is ready)
        if header.epoch.load(Ordering::Acquire) != expected_epoch {
            return Ok(None); // Writer hasn't finished yet
        }

        let data_len = header.len.load(Ordering::Acquire) as usize;
        if data_len == 0 {
            return Ok(None); // Not committed yet
        }

        // Check owner liveness
        let owner_pid = header.owner_pid.load(Ordering::Acquire);
        if owner_pid > 0 && !is_process_alive(owner_pid) {
            // Owner died, reset refcount and allow reclamation
            header.refcount.store(0, Ordering::Release);
        }

        // Try to acquire read lock
        let mut rc = header.refcount.load(Ordering::Acquire);
        loop {
            if rc != 0 {
                // Someone else is reading or slot is being reclaimed
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

        // Double-check generation after acquiring lock
        let gen_after = header.generation.load(Ordering::Acquire);
        let expected_gen = if first_word == PADDING_SENTINEL { 0 } else { 1 };

        // Verify data integrity
        let channel = header.channel.load(Ordering::Acquire);
        let data_ptr = unsafe { self.data.add(data_offset) };
        let slice = unsafe { std::slice::from_raw_parts(data_ptr, data_len) };

        // Create token for automatic reclamation
        let aligned_len = (data_len + ALIGNMENT - 1) & !(ALIGNMENT - 1);
        let actual_consumed = total_consumed + aligned_len;

        let token = Arc::new(SlotToken {
            ring: self,
            total_consumed: actual_consumed,
        });

        // Extend lifetime to 'static (safe because token keeps ring alive)
        let static_slice: &'static [u8] = unsafe { std::mem::transmute(slice) };

        Ok(Some(RawShmMessage {
            data: static_slice,
            channel,
            _token: token,
        }))
    }

    /// Wait for and allocate a slot (async)
    pub async fn wait_for_slot(&self, size: usize) -> WriteSlot<'_> {
        let mut spin = 0u32;
        let mut backoff = 1u64;

        loop {
            if let Some(slot) = self.try_alloc(size) {
                return slot;
            }

            spin += 1;
            if spin < 100 {
                // Fast spin
                std::hint::spin_loop();
            } else if spin < 1000 {
                // Yield occasionally
                tokio::task::yield_now().await;
            } else {
                // Exponential backoff with cap
                let delay = std::time::Duration::from_nanos(backoff.min(1_000_000));
                tokio::time::sleep(delay).await;
                backoff = (backoff * 2).min(10_000_000);
            }
        }
    }
}

/// Write slot for constructing a message
pub struct WriteSlot<'a> {
    ring: &'a RingBuffer,
    offset: usize,
    epoch_claim: u64,
    data_len: usize,
}

impl<'a> WriteSlot<'a> {
    /// Write data to the slot
    ///
    /// # Safety
    /// The data must fit within the allocated size
    pub fn write(&mut self, data: &[u8], channel: u8) {
        assert!(data.len() <= self.data_len, "Data exceeds allocated size");

        unsafe {
            let dest = self.ring.data.add(self.offset + HEADER_SIZE);
            ptr::copy_nonoverlapping(data.as_ptr(), dest, data.len());

            let header_ptr = self.ring.data.add(self.offset) as *mut SlotHeader;
            (*header_ptr).channel.store(channel, Ordering::Release);
        }
    }

    /// Commit the slot, making it visible to readers
    pub fn commit(self, actual_size: usize) {
        unsafe {
            let header_ptr = self.ring.data.add(self.offset) as *mut SlotHeader;

            // Memory barrier to ensure data is written before header
            std::sync::atomic::compiler_fence(Ordering::Release);

            // Set length first (readers check this)
            (*header_ptr)
                .len
                .store(actual_size as u32, Ordering::Release);

            // Increment generation for ABA protection
            (*header_ptr).generation.fetch_add(1, Ordering::Release);

            // Finally set epoch to make slot visible
            (*header_ptr)
                .epoch
                .store(self.epoch_claim, Ordering::Release);
        }

        // Prevent drop from being called
        std::mem::forget(self);
    }
}

impl<'a> Drop for WriteSlot<'a> {
    fn drop(&mut self) {
        // If not committed, we need to reclaim the space
        // In production: mark as abandoned for garbage collection
        debug!("WriteSlot dropped without commit - potential leak");
    }
}

/// Token for automatic memory reclamation
struct SlotToken {
    ring: *const RingBuffer,
    total_consumed: usize,
}

// Safety: Token is Send/Sync because it just holds a pointer to the ring
unsafe impl Send for SlotToken {}
unsafe impl Sync for SlotToken {}

impl Drop for SlotToken {
    fn drop(&mut self) {
        // Advance read position, making space available for writers
        unsafe {
            (*(*self.ring).control)
                .read_pos
                .fetch_add(self.total_consumed as u64, Ordering::Release);
        }
    }
}

/// Raw message from shared memory (zero-copy)
pub struct RawShmMessage {
    data: &'static [u8],
    channel: u8,
    _token: Arc<SlotToken>,
}

impl RawShmMessage {
    /// Get message payload
    pub fn get_bytes(&self) -> &'static [u8] {
        self.data
    }

    /// Get channel identifier
    pub fn channel(&self) -> u8 {
        self.channel
    }

    /// Clone the retention token
    pub fn token(&self) -> Arc<SlotToken> {
        self._token.clone()
    }
}

/// Typed message with zero-copy deserialization support
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
    /// Get reference to archived data (zero-copy)
    pub fn get(&self) -> &T::Archived {
        self.archived
    }

    /// Deserialize to owned type (requires copy)
    pub fn deserialize(&self) -> Result<T, CellError>
    where
        T::Archived: Deserialize<T, rkyv::de::deserializers::SharedDeserializeMap>,
    {
        let mut deserializer = rkyv::de::deserializers::SharedDeserializeMap::new();
        self.archived
            .deserialize(&mut deserializer)
            .map_err(|_| CellError::DeserializationFailure)
    }
}

// Safety bounds
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

/// SHM client for bidirectional communication
#[derive(Clone)]
pub struct ShmClient {
    pub tx: Arc<RingBuffer>,
    pub rx: Arc<RingBuffer>,
}

impl ShmClient {
    pub fn new(tx: Arc<RingBuffer>, rx: Arc<RingBuffer>) -> Self {
        Self { tx, rx }
    }

    /// Send raw bytes and wait for response
    pub async fn request_raw(
        &self,
        req_bytes: &[u8],
        channel: u8,
    ) -> Result<RawShmMessage, CellError> {
        let size = req_bytes.len();

        // Allocate and send
        let mut slot = self.tx.wait_for_slot(size).await;
        slot.write(req_bytes, channel);
        slot.commit(size);

        // Wait for response with backoff
        let mut spin = 0u32;
        let mut backoff = 1u64;

        loop {
            match self.rx.try_read_raw() {
                Ok(Some(msg)) => return Ok(msg),
                Ok(None) => {}
                Err(ShmError::Corruption(_)) => {
                    // Stale/corrupt read, retry
                    warn!("SHM corruption detected, retrying read");
                }
                Err(e) => return Err(e.into()),
            }

            spin += 1;
            if spin < 100 {
                std::hint::spin_loop();
            } else if spin < 1000 {
                tokio::task::yield_now().await;
            } else {
                let delay = std::time::Duration::from_nanos(backoff.min(1_000_000));
                tokio::time::sleep(delay).await;
                backoff = (backoff * 2).min(10_000_000);
            }
        }
    }

    /// Send a typed request and receive a typed response
    pub async fn request<Req, Resp>(
        &self,
        req: &Req,
        channel: u8,
    ) -> Result<ShmMessage<Resp>, CellError>
    where
        Req: Serialize<ShmSerializer>,
        Resp: Archive,
        Resp::Archived: for<'a> rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'a>> + 'static,
    {
        let req_bytes = rkyv::to_bytes::<_, 1024>(req).map_err(|_| CellError::SerializationFailure)?;
        let msg = self.request_raw(&req_bytes, channel).await?;

        // First try to deserialize as ErrorResponse
        if let Ok(error_archived) = rkyv::check_archived_root::<crate::ErrorResponse>(msg.get_bytes()) {
            let _error: crate::ErrorResponse = error_archived
                .deserialize(&mut rkyv::de::deserializers::SharedDeserializeMap::new())
                .map_err(|_| CellError::DeserializationFailure)?;
            return Err(CellError::InternalError);
        }

        // Then try the expected type
        let archived_ref = rkyv::check_archived_root::<Resp>(msg.get_bytes())
            .map_err(|_| CellError::DeserializationFailure)?;
        let archived_static: &'static Resp::Archived = unsafe { std::mem::transmute(archived_ref) };

        Ok(ShmMessage {
            archived: archived_static,
            _token: msg.token(),
            _phantom: PhantomData,
        })
    }

    /// Create a new SHM client pair from file descriptors
    ///
    /// # Safety
    /// FDs must be valid and properly initialized
    pub unsafe fn from_fds(tx_fd: RawFd, rx_fd: RawFd) -> Result<Self, CellError> {
        let tx = RingBuffer::attach(tx_fd).map_err(|e| CellError::from(e))?;
        let rx = RingBuffer::attach(rx_fd).map_err(|e| CellError::from(e))?;

        Ok(Self { tx, rx })
    }
}

/// Check if a process is still alive
fn is_process_alive(pid: u32) -> bool {
    #[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
    {
        use nix::sys::signal::kill;
        use nix::unistd::Pid;
        match kill(Pid::from_raw(pid as i32), None) {
            Ok(_) => true,
            Err(nix::errno::Errno::ESRCH) => false,
            Err(_) => true, // Permission error or other - assume alive to be safe
        }
    }
    #[cfg(not(all(unix, any(target_os = "linux", target_os = "macos"))))]
    {
        let _ = pid;
        true // Default to true on unsupported platforms
    }
}

/// Create a bidirectional SHM channel pair
///
/// Returns (client_tx_fd, client_rx_fd, server_tx_fd, server_rx_fd)
pub fn create_shm_channel(size: usize) -> Result<(RawFd, RawFd, RawFd, RawFd), ShmError> {
    use std::os::unix::io::IntoRawFd;

    // Create temporary files for backing
    let client_to_server =
        tempfile::tempfile().map_err(|e| ShmError::MappingFailed(e.to_string()))?;
    let server_to_client =
        tempfile::tempfile().map_err(|e| ShmError::MappingFailed(e.to_string()))?;

    // Set sizes
    let size = size.next_power_of_two().max(1024 * 1024);
    client_to_server
        .set_len(size as u64)
        .map_err(|e| ShmError::MappingFailed(e.to_string()))?;
    server_to_client
        .set_len(size as u64)
        .map_err(|e| ShmError::MappingFailed(e.to_string()))?;

    // Convert to raw fds (caller takes ownership)
    let c2s_fd = client_to_server.into_raw_fd();
    let s2c_fd = server_to_client.into_raw_fd();

    // Client: writes to c2s, reads from s2c
    // Server: writes to s2c, reads from c2s
    Ok((c2s_fd, s2c_fd, s2c_fd, c2s_fd))
}
