// examples/macro-db-sync/consumer/src/main.rs
use cell_sdk::expand;

// CONSUMER
// We don't know the fields of 'Product'.
// We just ask schema-hub for the definition of 'Product'.
#[expand("schema-hub", "shared_table")]
pub struct Product;

fn main() {
    println!("🛒 CONSUMER: Reading Synchronized Data...");

    // The compiler generated Product struct and ProductTable
    // based on what Producer defined!
    let db = ProductTable::new();
    let items = db.all();

    println!(
        "{:<4} | {:<20} | {:<10} | {}",
        "ID", "Name", "Price", "Stock"
    );
    println!("{}", "-".repeat(50));

    for item in items {
        // We can access fields like .name and .price even though we
        // never typed them in this file. They were injected at compile time.
        println!(
            "{:<4} | {:<20} | ${:<9.2} | {}",
            item.id,
            item.name,
            item.price,
            if item.in_stock { "YES" } else { "NO" }
        );
    }
}
