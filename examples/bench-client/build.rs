use std::env;
use std::path::PathBuf;

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    
    // Fetch schemas for both benchmark services
    if let Err(e) = cell_sdk::build::fetch_and_cache_schema("bench_echo", &out_dir) {
        println!("cargo:warning=⚠️  bench_echo not running: {}", e);
    }
    
    if let Err(e) = cell_sdk::build::fetch_and_cache_schema("bench_processor", &out_dir) {
        println!("cargo:warning=⚠️  bench_processor not running: {}", e);
    }
}
