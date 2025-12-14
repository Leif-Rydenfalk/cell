// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod bridge;
pub mod config;
pub mod error;
pub mod macro_coordination;
pub mod manifest;
pub mod ops;
pub mod protocol;
pub mod vesicle;

// Re-export common types for convenience
pub use error::Error;
// Re-export rkyv to ensure availability for derived traits
pub use rkyv;
