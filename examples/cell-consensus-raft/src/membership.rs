// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::Result;
use std::collections::HashSet;
use rkyv::{Archive, Serialize, Deserialize};

#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[archive(check_bytes)]
pub enum MembershipChange {
    AddNode { id: u64, address: String },
    RemoveNode { id: u64 },
}

pub struct MembershipManager {
    current_members: HashSet<u64>,
    pending_change: Option<MembershipChange>,
}

impl MembershipManager {
    pub fn new(initial_members: Vec<u64>) -> Self {
        Self {
            current_members: initial_members.into_iter().collect(),
            pending_change: None,
        }
    }

    pub fn propose_change(&mut self, change: MembershipChange) -> Result<()> {
        if self.pending_change.is_some() {
            anyhow::bail!("Cannot have multiple pending membership changes");
        }

        match &change {
            MembershipChange::AddNode { id, .. } => {
                if self.current_members.contains(id) {
                    anyhow::bail!("Node {} already exists", id);
                }
            }
            MembershipChange::RemoveNode { id } => {
                if !self.current_members.contains(id) {
                    anyhow::bail!("Node {} does not exist", id);
                }
            }
        }

        self.pending_change = Some(change);
        Ok(())
    }

    pub fn commit_change(&mut self) -> Result<()> {
        let change = self.pending_change.take()
            .ok_or_else(|| anyhow::anyhow!("No pending change"))?;

        match change {
            MembershipChange::AddNode { id, .. } => {
                self.current_members.insert(id);
            }
            MembershipChange::RemoveNode { id } => {
                self.current_members.remove(&id);
            }
        }

        Ok(())
    }

    pub fn members(&self) -> Vec<u64> {
        self.current_members.iter().copied().collect()
    }

    pub fn majority(&self) -> usize {
        (self.current_members.len() / 2) + 1
    }
}