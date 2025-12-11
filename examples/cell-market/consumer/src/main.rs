// Path: /Users/07lead01/cell/examples/cell-market/consumer/src/main.rs
use cell_sdk::cell_remote; 
use anyhow::Result;
use std::thread;
use std::time::Duration;

// RPC Connection (Standard)
cell_remote!(Ledger = "ledger");

// --- CELL MACRO EXPANSION ---
// This connects to the 'ledger' cell at compile time.
// It detects 'id' as the Primary Key automatically.
#[cell_sdk::expand("ledger", "table")]
struct Order {
    id: u64,
    symbol: String,
    amount: u64,
}

fn main() -> Result<()> {
    println!("--- Cell Macro Database Demo ---");
    
    // 1. Initialize the generated Table
    // The 'OrderTable' struct was created by the macro
    let db = OrderTable::new();

    // 2. Perform Database Operations
    println!("> Inserting 3 orders...");
    db.save(Order { id: 101, symbol: "BTC".into(), amount: 50 });
    db.save(Order { id: 102, symbol: "ETH".into(), amount: 200 });
    db.save(Order { id: 103, symbol: "SOL".into(), amount: 1000 });

    // 3. Thread-Safe Access check
    let db_clone = db.clone();
    let handle = thread::spawn(move || {
        // Accessing the same memory store from another thread
        if let Some(order) = db_clone.get(&102) {
            println!("  [Thread] Retrieved Order #{}: {} ({} units)", order.id, order.symbol, order.amount);
        }
        
        println!("  [Thread] Deleting Order #101");
        db_clone.remove(&101);
    });

    handle.join().unwrap();

    // 4. Verify Final State
    let all_orders = db.all();
    println!("\n> Final Database State (Count: {}):", db.count());
    for order in all_orders {
        println!("  - Order #{}: {} ({})", order.id, order.symbol, order.amount);
    }
    
    Ok(())
}