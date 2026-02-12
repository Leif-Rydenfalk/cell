// examples/macro-db-sync/producer/src/main.rs
use cell_sdk::expand;

// SOURCE OF TRUTH
// We define the fields here. The schema-hub will memorize them.
#[expand("schema-hub", "shared_table")]
pub struct Product {
    pub id: u64,
    pub name: String,
    pub price: f64,
    pub in_stock: bool,
}

fn main() {
    println!("🏭 PRODUCER: Defining Schema and Writing Data...");

    // Generated DAO
    let db = ProductTable::new();

    // Add some products
    let catalog = vec![
        Product {
            id: 1,
            name: "Neural Link".into(),
            price: 999.99,
            in_stock: true,
        },
        Product {
            id: 2,
            name: "Quantum Core".into(),
            price: 4500.00,
            in_stock: true,
        },
        Product {
            id: 3,
            name: "Legacy CPU".into(),
            price: 29.99,
            in_stock: false,
        },
    ];

    for p in catalog {
        db.save(p);
    }

    println!("✅ Data written to shared storage.");
}
