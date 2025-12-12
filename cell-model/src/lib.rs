// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub use cell_core::Vesicle;

pub mod protocol;
pub mod error;
pub mod vesicle;
pub mod ops;
pub mod macro_coordination;
pub mod bridge;
pub mod config;

pub use protocol::*;
pub use ops::*;
pub use error::Error;
// Explicitly export MacroCoordination types to avoid glob conflict with protocol::MacroKind
pub use macro_coordination::{
    MacroInfo, ExpansionContext, MacroCoordinationRequest, MacroCoordinationResponse
};
pub use bridge::*;
pub use config::*;

pub use rkyv;
pub use serde;