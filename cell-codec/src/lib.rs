// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

#![no_std]

extern crate alloc;

use rkyv::ser::serializers::AllocSerializer;
use rkyv::ser::Serializer;
use rkyv::Serialize;
use alloc::vec::Vec;

pub struct RkyvCodec;

impl RkyvCodec {
    pub fn encode<T>(item: &T) -> Result<Vec<u8>, &'static str>
    where
        T: Serialize<AllocSerializer<1024>>,
    {
        let mut serializer = AllocSerializer::<1024>::default();
        serializer.serialize_value(item).map_err(|_| "Serialization failed")?;
        Ok(serializer.into_serializer().into_inner().into_vec())
    }
}