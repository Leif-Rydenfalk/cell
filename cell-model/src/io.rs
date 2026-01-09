// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use alloc::string::String;
use rkyv::{Archive, Deserialize, Serialize};

#[derive(Archive, Serialize, Deserialize, Debug)]
#[archive(check_bytes)]
pub enum IoRequest {
    /// "I am cell 'worker-1'. Please bind my Membrane."
    Bind { cell_name: String },

    /// "I want to talk to 'ledger'. Give me a connection."
    Connect { target_cell: String },
}

#[derive(Archive, Serialize, Deserialize, Debug)]
#[archive(check_bytes)]
pub enum IoResponse {
    /// "Here is your FD. It is a Unix Listener."
    ListenerBound,

    /// "Here are your FDs (Read/Write) or SHM FDs."
    ConnectionEstablished,

    Error {
        message: String,
    },
}
