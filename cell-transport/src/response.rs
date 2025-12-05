// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

#[cfg(target_os = "linux")]
use crate::shm::ShmMessage;
use rkyv::{Archive, Deserialize};
use std::marker::PhantomData;
use anyhow::Result;

// Response now carries a lifetime 'a to allow borrowing from Synapse buffer
pub enum Response<'a, T: Archive>
where
    <T as Archive>::Archived: 'static,
{
    Owned(Vec<u8>), // Network / Legacy
    Borrowed(&'a [u8]), // Zero-Copy Socket Read
    #[cfg(target_os = "linux")]
    ZeroCopy(ShmMessage<T>), // Zero-Copy SHM
    #[cfg(not(target_os = "linux"))]
    _Phantom(PhantomData<&'a T>),
}

impl<'a, T: Archive> Response<'a, T>
where
    <T as Archive>::Archived: 'static,
{
    /// Access the archived data without deserializing.
    /// This is the fastest way to read data.
    pub fn get(&self) -> Result<&T::Archived>
    where
        T::Archived: for<'b> rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'b>>,
    {
        // Helper to validate root
        fn validate_root<'b, U: Archive>(
            bytes: &'b [u8],
            context: &str,
        ) -> Result<&'b U::Archived>
        where
            U::Archived: rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'b>>,
        {
             rkyv::check_archived_root::<U>(bytes).map_err(|e| {
                anyhow::anyhow!(
                    "Invalid data format in {}: {:?} (len: {})",
                    context,
                    e,
                    bytes.len()
                )
            })
        }

        match self {
            Response::Owned(bytes) => validate_root::<T>(bytes, "Response::get"),
            Response::Borrowed(bytes) => validate_root::<T>(bytes, "Response::get"),
            #[cfg(target_os = "linux")]
            Response::ZeroCopy(msg) => Ok(msg.get()),
            #[cfg(not(target_os = "linux"))]
            Response::_Phantom(_) => anyhow::bail!("Invalid state"),
        }
    }

    /// Deserializes the data into a standard Rust struct.
    /// This performs a deep copy and allocation.
    pub fn deserialize(&self) -> Result<T>
    where
        T::Archived: Deserialize<T, rkyv::de::deserializers::SharedDeserializeMap>
            + for<'b> rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'b>>,
    {
        let archived: &T::Archived = self.get()?;
        let mut deserializer = rkyv::de::deserializers::SharedDeserializeMap::new();
        Ok(archived.deserialize(&mut deserializer)?)
    }
}