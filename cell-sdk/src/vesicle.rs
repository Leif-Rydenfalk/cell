/// A container for payload data.
///
/// It acts as a Zero-Copy abstraction over:
/// 1. Owned Memory (Heap/Socket buffers)
/// 2. Shared Memory (Ring Buffer Locks)
///
/// It implements `Deref<Target=[u8]>`, so you can use it like a slice.
pub enum Vesicle<'a> {
    /// Standard heap-allocated buffer (Socket transport)
    Owned(Vec<u8>),

    /// Zero-copy reference to the Shared Memory Ring Buffer.
    /// Holding this variant keeps the consumer lock active via the RAII guard.
    #[cfg(target_os = "linux")]
    Borrowed(&'a [u8]),

    /// Fallback for non-linux or empty states
    Empty,
}

impl<'a> Vesicle<'a> {
    /// Wraps an owned vector.
    pub fn wrap(data: Vec<u8>) -> Self {
        Self::Owned(data)
    }

    /// Pre-allocate capacity
    pub fn with_capacity(size: usize) -> Self {
        Self::Owned(vec![0u8; size])
    }

    /// Returns a slice to the underlying data.
    pub fn as_slice(&self) -> &[u8] {
        match self {
            Self::Owned(vec) => vec.as_slice(),
            #[cfg(target_os = "linux")]
            Self::Borrowed(slice) => slice,
            Self::Empty => &[],
        }
    }

    /// Get mutable slice (only for Owned variant)
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        match self {
            Self::Owned(vec) => vec.as_mut_slice(),
            _ => panic!("Cannot get mutable slice from borrowed vesicle"),
        }
    }

    pub fn len(&self) -> usize {
        self.as_slice().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// Allow treating Vesicle directly as a byte slice
impl<'a> std::ops::Deref for Vesicle<'a> {
    type Target = [u8];
    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl<'a> std::fmt::Debug for Vesicle<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Vesicle(len={})", self.len())
    }
}
