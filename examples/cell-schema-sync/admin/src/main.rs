// examples/cell-schema-sync/admin/src/main.rs
use anyhow::Result;
use cell_sdk::expand;

// The Consumer
// We DO NOT define fields here. We just name the struct.
// The `expand` macro asks 'database' for the definition of 'User'.
// It injects the EXACT same code generated for user-management.
#[expand("database", "table")]
struct User; 

fn main() -> Result<()> {
    println!("--- Admin Cell ---");
    
    // We can use UserTable and User as if we defined them
    let table = UserTable::new();
    
    // We can instantiate User even though we wrote `struct User;`
    // because the macro replaced it with the full definition!
    let u = User {
        user_id: 99,
        username: "admin".to_string(),
        email: "admin@cell.foundation".to_string(),
    };
    
    table.save(u);
    
    let retrieved = table.get(&99).unwrap();
    println!("Admin User: {} ({})", retrieved.username, retrieved.email);
    
    Ok(())
}