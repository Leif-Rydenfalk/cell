// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const STATE_CLOSED: u8 = 0;
const STATE_OPEN: u8 = 1;
const STATE_HALF_OPEN: u8 = 2;

pub struct CircuitBreaker {
    failure_threshold: u64,
    success_threshold: u64,
    timeout: Duration,
    
    state: AtomicU8,
    failure_count: AtomicU64,
    success_count: AtomicU64,
    last_failure_time: AtomicU64,
}

impl CircuitBreaker {
    pub fn new(failure_threshold: u64, timeout: Duration) -> Arc<Self> {
        Arc::new(Self {
            failure_threshold,
            success_threshold: 2,
            timeout,
            state: AtomicU8::new(STATE_CLOSED),
            failure_count: AtomicU64::new(0),
            success_count: AtomicU64::new(0),
            last_failure_time: AtomicU64::new(0),
        })
    }

    pub fn call<F, T>(&self, f: F) -> Result<T, CircuitBreakerError>
    where
        F: FnOnce() -> Result<T, anyhow::Error>,
    {
        match self.state.load(Ordering::Acquire) {
            STATE_OPEN => {
                let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
                let last_failure = self.last_failure_time.load(Ordering::Acquire);
                
                if now - last_failure >= self.timeout.as_secs() {
                    self.state.store(STATE_HALF_OPEN, Ordering::Release);
                    self.success_count.store(0, Ordering::Release);
                } else {
                    return Err(CircuitBreakerError::Open);
                }
            }
            _ => {}
        }

        match f() {
            Ok(result) => {
                self.on_success();
                Ok(result)
            }
            Err(e) => {
                self.on_failure();
                Err(CircuitBreakerError::Execution(e))
            }
        }
    }

    fn on_success(&self) {
        let state = self.state.load(Ordering::Acquire);
        
        if state == STATE_HALF_OPEN {
            let successes = self.success_count.fetch_add(1, Ordering::AcqRel) + 1;
            if successes >= self.success_threshold {
                self.state.store(STATE_CLOSED, Ordering::Release);
                self.failure_count.store(0, Ordering::Release);
            }
        } else {
            self.failure_count.store(0, Ordering::Release);
        }
    }

    fn on_failure(&self) {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        self.last_failure_time.store(now, Ordering::Release);
        
        let failures = self.failure_count.fetch_add(1, Ordering::AcqRel) + 1;
        if failures >= self.failure_threshold {
            self.state.store(STATE_OPEN, Ordering::Release);
        }
    }

    pub fn is_open(&self) -> bool {
        self.state.load(Ordering::Acquire) == STATE_OPEN
    }
}

#[derive(Debug)]
pub enum CircuitBreakerError {
    Open,
    Execution(anyhow::Error),
}

impl std::fmt::Display for CircuitBreakerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open => write!(f, "Circuit breaker is open"),
            Self::Execution(e) => write!(f, "Execution failed: {}", e),
        }
    }
}

impl std::error::Error for CircuitBreakerError {}