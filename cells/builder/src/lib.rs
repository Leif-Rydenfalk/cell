// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

// Heavy logic moved to 'cells/builder' and 'cells/hypervisor'
// This crate now just defines the Root daemon logic.

pub mod root;
pub use root::MyceliumRoot;