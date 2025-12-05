// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod vesicle;
pub mod protocol;
pub mod error;

pub use vesicle::Vesicle;
pub use protocol::*;
pub use error::Error;

// Re-export for macros/dependencies
pub use rkyv;
pub use serde;