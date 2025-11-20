use rkyv::AlignedVec;

/// A Vesicle is a membrane-bound container for data (Proteins).
/// It ensures data is aligned for zero-copy processing.
pub struct Vesicle {
    inner: AlignedVec,
}

impl Vesicle {
    pub fn new() -> Self {
        Self {
            inner: AlignedVec::new(),
        }
    }

    pub fn with_capacity(cap: usize) -> Self {
        let mut inner = AlignedVec::with_capacity(cap);
        // Pre-expand logic to allow direct read
        for _ in 0..cap {
            inner.push(0);
        }
        Self { inner }
    }

    pub fn wrap(data: Vec<u8>) -> Self {
        let mut inner = AlignedVec::with_capacity(data.len());
        inner.extend_from_slice(&data);
        Self { inner }
    }

    pub fn as_slice(&self) -> &[u8] {
        self.inner.as_slice()
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        self.inner.as_mut_slice()
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Extract the inner AlignedVec
    pub fn into_inner(self) -> AlignedVec {
        self.inner
    }
}
