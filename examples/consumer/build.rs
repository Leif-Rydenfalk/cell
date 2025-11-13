use std::env;
use std::path::PathBuf;

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    
    // Fetch schema from running calculator service
    if let Err(e) = cell_sdk::build::fetch_and_cache_schema("calculator", &out_dir) {
        // If service not running, check if we have cached schema
        let schema_path = out_dir.join("calculator_schema.json");
        if !schema_path.exists() {
            panic!("\n\n❌ Cannot build without calculator schema!\n\
                    \n\
                    Error: {}\n\
                    \n\
                    To fix:\n\
                    1. Start calculator: ./target/release/cell start calculator ./target/release/calculator\n\
                    2. Rebuild this crate\n\
                    \n\
                    Or use: cell prepare calculator\n\
                    \n", e);
        } else {
            println!("cargo:warning=⚠️  Using cached schema (calculator not running)");
        }
    }
}
