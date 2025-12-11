// Path: /Users/07lead01/cell/examples/cell-market/consumer/src/main.rs
use cell_sdk::cell_remote; 
use anyhow::Result;
use std::thread;

// RPC Connection (Standard)
cell_remote!(ledger = "ledger");

// --- CELL MACRO EXPANSION (SIMULATED) ---
// Since the `expand` macro requires the target cell ('ledger') to be running
// to perform the handshake and code generation, we cannot use it in a 
// cold compilation test environment.
// 
// In production, you would use:
// #[cell_sdk::expand("ledger", "table")]
// struct Order { ... }
//
// Here we manually expand the code that would be generated.

#[derive(Clone, Debug, PartialEq, 
    cell_sdk::serde::Serialize, cell_sdk::serde::Deserialize,
    cell_sdk::rkyv::Archive, cell_sdk::rkyv::Serialize, cell_sdk::rkyv::Deserialize
)]
#[archive(check_bytes)]
#[archive(crate = "cell_sdk::rkyv")]
#[serde(crate = "cell_sdk::serde")]
pub struct Order {
    pub id: u64,
    pub symbol: String,
    pub amount: u64,
}

#[derive(Clone)]
pub struct OrderTable {
    storage: std::sync::Arc<std::sync::RwLock<std::collections::HashMap<u64, Order>>>,
}

impl OrderTable {
    pub fn new() -> Self {
        Self {
            storage: std::sync::Arc::new(std::sync::RwLock::new(std::collections::HashMap::new())),
        }
    }

    pub fn save(&self, item: Order) {
        let mut guard = self.storage.write().unwrap();
        guard.insert(item.id.clone(), item);
    }

    pub fn get(&self, id: &u64) -> Option<Order> {
        let guard = self.storage.read().unwrap();
        guard.get(id).cloned()
    }

    pub fn remove(&self, id: &u64) -> Option<Order> {
        let mut guard = self.storage.write().unwrap();
        guard.remove(id)
    }

    pub fn all(&self) -> Vec<Order> {
        let guard = self.storage.read().unwrap();
        guard.values().cloned().collect()
    }

    pub fn count(&self) -> usize {
        let guard = self.storage.read().unwrap();
        guard.len()
    }
}

fn main() -> Result<()> {
    println!("--- Cell Macro Database Demo (Simulated Expansion) ---");
    
    let db = OrderTable::new();

    println!("> Inserting 3 orders...");
    db.save(Order { id: 101, symbol: "BTC".into(), amount: 50 });
    db.save(Order { id: 102, symbol: "ETH".into(), amount: 200 });
    db.save(Order { id: 103, symbol: "SOL".into(), amount: 1000 });

    let db_clone = db.clone();
    let handle = thread::spawn(move || {
        if let Some(order) = db_clone.get(&102) {
            println!("  [Thread] Retrieved Order #{}: {} ({} units)", order.id, order.symbol, order.amount);
        }
        
        println!("  [Thread] Deleting Order #101");
        db_clone.remove(&101);
    });

    handle.join().unwrap();

    let all_orders = db.all();
    println!("\n> Final Database State (Count: {}):", db.count());
    for order in all_orders {
        println!("  - Order #{}: {} ({})", order.id, order.symbol, order.amount);
    }
    
    Ok(())
}