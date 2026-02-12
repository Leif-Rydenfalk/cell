// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, Semaphore};
use anyhow::Result;
use crate::synapse::Synapse;

pub struct ConnectionPool {
    connections: Arc<RwLock<HashMap<String, Vec<Synapse>>>>,
    max_per_cell: usize,
    semaphore: Arc<Semaphore>,
}

impl ConnectionPool {
    pub fn new(max_per_cell: usize, max_total: usize) -> Self {
        Self {
            connections: Arc::new(RwLock::new(HashMap::new())),
            max_per_cell,
            semaphore: Arc::new(Semaphore::new(max_total)),
        }
    }

    pub async fn acquire(&self, cell_name: &str) -> Result<Synapse> {
        // Try to reuse existing connection
        {
            let mut conns = self.connections.write().await;
            if let Some(pool) = conns.get_mut(cell_name) {
                if let Some(conn) = pool.pop() {
                    return Ok(conn);
                }
            }
        }

        // Create new connection
        let _permit = self.semaphore.acquire().await?;
        let conn = Synapse::grow(cell_name).await?;
        Ok(conn)
    }

    pub async fn release(&self, cell_name: String, conn: Synapse) {
        let mut conns = self.connections.write().await;
        let pool = conns.entry(cell_name).or_insert_with(Vec::new);
        
        if pool.len() < self.max_per_cell {
            pool.push(conn);
        }
        // Otherwise drop connection (destructor handles cleanup)
    }
}