// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration};
use cell_macros::protein;

pub struct Metrics {
    // Request metrics
    pub requests_total: AtomicU64,
    pub requests_success: AtomicU64,
    pub requests_failed: AtomicU64,
    
    // Latency histogram (microseconds)
    pub latency_buckets: [AtomicU64; 10], // <1ms, <5ms, <10ms, <50ms, <100ms, <500ms, <1s, <5s, <10s, >10s
    
    // Connection metrics
    pub connections_active: AtomicU64,
    pub connections_total: AtomicU64,
    pub reconnects: AtomicU64,
    
    // Transport metrics
    pub bytes_sent: AtomicU64,
    pub bytes_received: AtomicU64,
}

impl Metrics {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            requests_total: AtomicU64::new(0),
            requests_success: AtomicU64::new(0),
            requests_failed: AtomicU64::new(0),
            latency_buckets: Default::default(),
            connections_active: AtomicU64::new(0),
            connections_total: AtomicU64::new(0),
            reconnects: AtomicU64::new(0),
            bytes_sent: AtomicU64::new(0),
            bytes_received: AtomicU64::new(0),
        })
    }

    pub fn record_request(&self, duration: Duration, success: bool) {
        self.requests_total.fetch_add(1, Ordering::Relaxed);
        
        if success {
            self.requests_success.fetch_add(1, Ordering::Relaxed);
        } else {
            self.requests_failed.fetch_add(1, Ordering::Relaxed);
        }

        let micros = duration.as_micros() as u64;
        let bucket = match micros {
            0..=1_000 => 0,
            1_001..=5_000 => 1,
            5_001..=10_000 => 2,
            10_001..=50_000 => 3,
            50_001..=100_000 => 4,
            100_001..=500_000 => 5,
            500_001..=1_000_000 => 6,
            1_000_001..=5_000_000 => 7,
            5_000_001..=10_000_000 => 8,
            _ => 9,
        };
        self.latency_buckets[bucket].fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            requests_total: self.requests_total.load(Ordering::Relaxed),
            requests_success: self.requests_success.load(Ordering::Relaxed),
            requests_failed: self.requests_failed.load(Ordering::Relaxed),
            latency_histogram: self.latency_buckets.iter().map(|b| b.load(Ordering::Relaxed)).collect(),
            connections_active: self.connections_active.load(Ordering::Relaxed),
            bytes_sent: self.bytes_sent.load(Ordering::Relaxed),
            bytes_received: self.bytes_received.load(Ordering::Relaxed),
        }
    }
}

#[protein]
pub struct MetricsSnapshot {
    pub requests_total: u64,
    pub requests_success: u64,
    pub requests_failed: u64,
    pub latency_histogram: Vec<u64>,
    pub connections_active: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
}

impl MetricsSnapshot {
    pub fn success_rate(&self) -> f64 {
        if self.requests_total == 0 { return 0.0; }
        (self.requests_success as f64 / self.requests_total as f64) * 100.0
    }

    pub fn p50_latency(&self) -> &'static str {
        self.percentile_bucket(50)
    }

    pub fn p99_latency(&self) -> &'static str {
        self.percentile_bucket(99)
    }

    fn percentile_bucket(&self, p: u64) -> &'static str {
        let total: u64 = self.latency_histogram.iter().sum();
        if total == 0 { return "N/A"; }
        
        let target = (total * p) / 100;
        let mut cumulative = 0u64;
        
        let labels = ["<1ms", "<5ms", "<10ms", "<50ms", "<100ms", "<500ms", "<1s", "<5s", "<10s", ">10s"];
        
        for (i, &count) in self.latency_histogram.iter().enumerate() {
            cumulative += count;
            if cumulative >= target {
                return labels[i];
            }
        }
        ">10s"
    }
}