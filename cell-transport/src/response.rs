// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

#[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
use crate::shm::ShmMessage;
use rkyv::{Archive, Deserialize};
use std::marker::PhantomData;
use anyhow::Result;

pub enum Response<'a, T: Archive>
where
    <T as Archive>::Archived: 'static,
{
    Owned(Vec<u8>),
    Borrowed(&'a [u8]),
    
    #[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
    ZeroCopy(ShmMessage<T>),
    
    #[cfg(not(all(feature = "shm", any(target_os = "linux", target_os = "macos"))))]
    _Phantom(PhantomData<&'a T>),
}

impl<'a, T: Archive> Response<'a, T>
where
    <T as Archive>::Archived: 'static,
{
    pub fn get(&self) -> Result<&T::Archived>
    where
        T::Archived: for<'b> rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'b>>,
    {
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
            
            #[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
            Response::ZeroCopy(msg) => Ok(msg.get()),
            
            #[cfg(not(all(feature = "shm", any(target_os = "linux", target_os = "macos"))))]
            Response::_Phantom(_) => anyhow::bail!("Invalid state"),
        }
    }

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