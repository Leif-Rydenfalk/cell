// SPDX-License-Identifier: MIT
// Distributed State Primitives

use std::collections::HashMap;
// Fixed: Removed unused imports
use cell_macros::protein;

/// A G-Counter (Grow-only Counter)
#[protein]
pub struct GCounter {
    id: u64, // Node ID
    counts: HashMap<u64, u64>,
}

impl GCounter {
    pub fn new(node_id: u64) -> Self {
        Self {
            id: node_id,
            counts: HashMap::new(),
        }
    }

    pub fn inc(&mut self) {
        *self.counts.entry(self.id).or_insert(0) += 1;
    }

    pub fn value(&self) -> u64 {
        self.counts.values().sum()
    }

    pub fn merge(&mut self, other: &GCounter) {
        for (node, count) in &other.counts {
            let entry = self.counts.entry(*node).or_insert(0);
            *entry = std::cmp::max(*entry, *count);
        }
    }
}

/// A Replicated Register (Last-Write-Wins)
#[protein]
pub struct LwwRegister<T> {
    value: T,
    timestamp: u64,
}

impl<T: Clone> LwwRegister<T> {
    pub fn new(value: T, timestamp: u64) -> Self {
        Self { value, timestamp }
    }

    pub fn set(&mut self, value: T, timestamp: u64) {
        if timestamp > self.timestamp {
            self.value = value;
            self.timestamp = timestamp;
        }
    }

    pub fn get(&self) -> &T {
        &self.value
    }

    pub fn merge(&mut self, other: &LwwRegister<T>) {
        if other.timestamp > self.timestamp {
            self.value = other.value.clone();
            self.timestamp = other.timestamp;
        }
    }
}