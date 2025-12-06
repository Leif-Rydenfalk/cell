// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use alloc::string::String;
use serde::{Deserialize, Serialize};
use rkyv::{Archive, Serialize as RkyvSerialize, Deserialize as RkyvDeserialize};

#[derive(Archive, RkyvSerialize, RkyvDeserialize, Serialize, Deserialize, Debug, Clone)]
#[archive(check_bytes)]
pub enum OpsRequest {
    /// Basic liveness check
    Ping,
    /// Request internal status (uptime, stats)
    Status,
}

#[derive(Archive, RkyvSerialize, RkyvDeserialize, Serialize, Deserialize, Debug, Clone)]
#[archive(check_bytes)]
pub enum OpsResponse {
    Pong,
    Status {
        name: String,
        uptime_secs: u64,
        memory_usage: u64,
        consensus_role: String,
    },
}