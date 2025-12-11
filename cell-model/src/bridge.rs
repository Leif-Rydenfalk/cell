// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use alloc::string::String;
use rkyv::{Archive, Serialize as RkyvSerialize, Deserialize as RkyvDeserialize};
use serde::{Deserialize, Serialize};

#[derive(Archive, RkyvSerialize, RkyvDeserialize, Serialize, Deserialize, Debug, Clone)]
#[archive(check_bytes)]
pub enum BridgeRequest {
    /// "I want to talk to this target. Make it exist locally."
    /// target: can be a simple name "ledger" or a URI "mavlink:drone"
    Mount { target: String },
}

#[derive(Archive, RkyvSerialize, RkyvDeserialize, Serialize, Deserialize, Debug, Clone)]
#[archive(check_bytes)]
pub enum BridgeResponse {
    /// "Ready. Connect to this Unix socket path."
    Mounted { socket_path: String },
    /// "I don't know how to reach that target."
    NotFound,
    /// "Something went wrong."
    Error { message: String },
}