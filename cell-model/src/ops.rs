// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use alloc::string::String;
use alloc::vec::Vec;
use rkyv::{Archive, Deserialize as RkyvDeserialize, Serialize as RkyvSerialize};
use serde::{Deserialize, Serialize};

#[derive(Archive, RkyvSerialize, RkyvDeserialize, Serialize, Deserialize, Debug, Clone)]
#[archive(check_bytes)]
pub enum OpsRequest {
    /// Basic liveness check
    Ping,
    /// Request internal status (uptime, stats)
    Status,
    /// Request Metrics Snapshot
    Metrics,
    /// Graceful Shutdown
    Shutdown,
    /// Fetch the source code of this cell for remote client generation
    GetSource,
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
    Metrics(MetricsSnapshot),
    ShutdownAck,
    Source {
        bytes: Vec<u8>,
    },
}

#[derive(Archive, RkyvSerialize, RkyvDeserialize, Serialize, Deserialize, Debug, Clone)]
#[archive(check_bytes)]
pub struct MetricsSnapshot {
    pub requests_total: u64,
    pub requests_success: u64,
    pub requests_failed: u64,
    pub latency_histogram: Vec<u64>,
    pub connections_active: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
}
