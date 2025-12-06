// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use std::time::Duration;
use tokio::time::timeout;
use anyhow::Result;

#[derive(Clone, Debug)]
pub struct Deadline {
    timeout: Duration,
}

impl Deadline {
    pub fn new(timeout: Duration) -> Self {
        Self { timeout }
    }

    pub async fn execute<F, T>(&self, f: F) -> Result<T>
    where
        F: std::future::Future<Output = Result<T>>,
    {
        match timeout(self.timeout, f).await {
            Ok(result) => result,
            Err(_) => anyhow::bail!("Request deadline exceeded ({:?})", self.timeout),
        }
    }
}