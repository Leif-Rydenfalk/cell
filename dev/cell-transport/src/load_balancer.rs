// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

pub enum LoadBalanceStrategy {
    RoundRobin,
    LeastConnections,
    Random,
}

pub struct LoadBalancer {
    strategy: LoadBalanceStrategy,
    counter: AtomicUsize,
}

impl LoadBalancer {
    pub fn new(strategy: LoadBalanceStrategy) -> Arc<Self> {
        Arc::new(Self {
            strategy,
            counter: AtomicUsize::new(0),
        })
    }

    pub fn select(&self, candidates: &[String]) -> Option<String> {
        if candidates.is_empty() {
            return None;
        }

        match self.strategy {
            LoadBalanceStrategy::RoundRobin => {
                let idx = self.counter.fetch_add(1, Ordering::Relaxed) % candidates.len();
                Some(candidates[idx].clone())
            }
            
            LoadBalanceStrategy::Random => {
                use rand::Rng;
                let idx = rand::thread_rng().gen_range(0..candidates.len());
                Some(candidates[idx].clone())
            }
            
            LoadBalanceStrategy::LeastConnections => {
                // TODO: Track active connections per node
                // For now, fallback to round-robin
                let idx = self.counter.fetch_add(1, Ordering::Relaxed) % candidates.len();
                Some(candidates[idx].clone())
            }
        }
    }
}