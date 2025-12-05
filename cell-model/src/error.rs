// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use alloc::string::String;
use core::fmt;

#[derive(Debug)]
pub enum Error {
    Serialization(String),
    Validation(String),
    Protocol(String),
    Transport(String),
    Other(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Serialization(s) => write!(f, "Serialization error: {}", s),
            Error::Validation(s) => write!(f, "Validation error: {}", s),
            Error::Protocol(s) => write!(f, "Protocol error: {}", s),
            Error::Transport(s) => write!(f, "Transport error: {}", s),
            Error::Other(s) => write!(f, "Error: {}", s),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}