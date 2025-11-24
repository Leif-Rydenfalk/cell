use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use crate::sys_log;

// The currency of the Cell Network
pub type ATP = i64;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Transaction {
    pub timestamp: u64,
    pub peer_id: String,
    pub job_id: String,
    pub cpu_ms: u64,
    pub amount: ATP,
    pub direction: TxDirection,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum TxDirection {
    Earned, // I ran a job for someone
    Spent,  // Someone ran a job for me
}

#[derive(Serialize, Deserialize, Default, Debug)]
struct Ledger {
    balance: ATP,
    history: Vec<Transaction>,
}

pub struct Mitochondria {
    ledger_path: PathBuf,
    state: Mutex<Ledger>,
}

impl Mitochondria {
    pub fn load_or_init(root: &Path) -> Result<Self> {
        let ledger_path = root.join("mitochondria.json");
        let state = if ledger_path.exists() {
            let data = fs::read_to_string(&ledger_path)?;
            serde_json::from_str(&data).unwrap_or_default()
        } else {
            Ledger::default()
        };

        Ok(Self {
            ledger_path,
            state: Mutex::new(state),
        })
    }

    /// Calculate the cost of work.
    /// Formula: 1 ATP per 100ms of CPU time.
    pub fn calculate_cost(&self, cpu_ms: u64) -> ATP {
        let cost = (cpu_ms as f64 / 100.0).ceil() as i64;
        std::cmp::max(1, cost) // Minimum 1 ATP
    }

    /// Record that we worked for someone (Earning ATP).
    pub fn synthesize_atp(&self, peer_id: &str, job_id: &str, cpu_ms: u64) -> Result<ATP> {
        let amount = self.calculate_cost(cpu_ms);
        let mut ledger = self.state.lock().unwrap();

        ledger.balance += amount;
        ledger.history.push(Transaction {
            timestamp: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)?
                .as_secs(),
            peer_id: peer_id.to_string(),
            job_id: job_id.to_string(),
            cpu_ms,
            amount,
            direction: TxDirection::Earned,
        });

        self.save(&ledger)?;
        sys_log(
            "FINANCE",
            &format!("Generated {} ATP. New Balance: {}", amount, ledger.balance),
        );
        Ok(amount)
    }

    /// Record that someone worked for us (Spending ATP).
    pub fn burn_atp(&self, peer_id: &str, job_id: &str, cpu_ms: u64) -> Result<ATP> {
        let amount = self.calculate_cost(cpu_ms);
        let mut ledger = self.state.lock().unwrap();

        ledger.balance -= amount;
        ledger.history.push(Transaction {
            timestamp: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)?
                .as_secs(),
            peer_id: peer_id.to_string(),
            job_id: job_id.to_string(),
            cpu_ms,
            amount,
            direction: TxDirection::Spent,
        });

        self.save(&ledger)?;
        sys_log(
            "FINANCE",
            &format!(
                "Spent {} ATP. Remaining Balance: {}",
                amount, ledger.balance
            ),
        );
        Ok(amount)
    }

    pub fn get_balance(&self) -> ATP {
        self.state.lock().unwrap().balance
    }

    pub fn print_statement(&self) {
        let ledger = self.state.lock().unwrap();
        println!("\n=== MITOCHONDRIA STATEMENT ===");
        println!("Current Energy Level: {} ATP", ledger.balance);
        println!("Transaction History:");
        println!(
            "{:<20} | {:<15} | {:<10} | {:<10} | {:<10}",
            "Time", "Peer", "CPU(ms)", "ATP", "Type"
        );
        println!("{}", "-".repeat(75));

        // Show last 10
        for tx in ledger.history.iter().rev().take(10) {
            println!(
                "{:<20} | {:<15} | {:<10} | {:<10} | {:?}",
                tx.timestamp,
                &tx.peer_id[0..8],
                tx.cpu_ms,
                tx.amount,
                tx.direction
            );
        }
        println!("==============================\n");
    }

    fn save(&self, ledger: &Ledger) -> Result<()> {
        let data = serde_json::to_string_pretty(ledger)?;
        fs::write(&self.ledger_path, data)?;
        Ok(())
    }
}
