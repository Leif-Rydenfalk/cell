// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

// We re-export Vesicle from Core to maintain API compatibility
pub use cell_core::Vesicle;

pub mod protocol;
pub mod error;
pub mod vesicle;
pub mod ops;
pub mod macro_coordination;

pub use protocol::*;
pub use ops::*;
pub use error::Error;
pub use macro_coordination::*;

pub use rkyv;
pub use serde;