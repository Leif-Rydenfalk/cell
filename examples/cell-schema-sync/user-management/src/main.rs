// examples/cell-schema-sync/user-management/src/main.rs
use anyhow::Result;
use cell_sdk::expand;

// The Source of Truth
// We define the struct fields here.
// The `expand` macro sends this definition to 'database' at compile time.
#[expand("database", "table")]
pub struct User {
    pub user_id: u64,
    pub username: String,
    pub email: String,
}

fn main() -> Result<()> {
    println!("--- User Management Cell ---");
    
    // The macro generated UserTable automatically
    let table = UserTable::new();
    
    println!("Creating User...");
    let u = User {
        user_id: 1,
        username: "alice".to_string(),
        email: "alice@example.com".to_string(),
    };
    
    table.save(u);
    println!("User saved locally via generated DAO.");
    
    Ok(())
}