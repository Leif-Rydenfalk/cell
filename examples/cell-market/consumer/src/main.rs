// In examples/cell-market/consumer/src/main.rs
use cell_sdk::cell_remote; 

// Use the macro with import_macros = true
cell_remote!(Ledger = "ledger", import_macros = true);

// Use the macro provided by Ledger
#[Ledger::table]
struct MyTable {
    id: u64,
    data: String,
}

fn main() -> Result<()> {
    println!("Consumer compiled successfully with Ledger macros!");
    Ok(())
}