// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use alloc::string::String;
use core::fmt;

#[derive(Debug)]
pub enum Error {
    Protocol(String),
    Transport(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Protocol(s) => write!(f, "Protocol error: {}", s),
            Error::Transport(s) => write!(f, "Transport error: {}", s),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}