// cell-cli/build.rs
// SPDX-License-Identifier: MIT
// Auto-register cells in the registry to ensure the Builder can find them

use std::fs;
use std::path::Path;

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let root_dir = Path::new(&manifest_dir).parent().unwrap();
    let registry_dir = dirs::home_dir().expect("No HOME").join(".cell/registry");

    if !registry_dir.exists() {
        fs::create_dir_all(&registry_dir).expect("Failed to create registry dir");
    }

    let cells_dir = root_dir.join("cells");
    if cells_dir.exists() {
        if let Ok(entries) = fs::read_dir(cells_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() && path.join("Cargo.toml").exists() {
                    let name = path.file_name().unwrap();
                    let link_path = registry_dir.join(name);
                    
                    if !link_path.exists() {
                        #[cfg(unix)]
                        let _ = std::os::unix::fs::symlink(&path, &link_path);
                    }
                }
            }
        }
    }
    
    // Also register examples if needed
    let examples_dir = root_dir.join("examples/cell-schema-sync");
    if examples_dir.exists() {
         if let Ok(entries) = fs::read_dir(examples_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() && path.join("Cargo.toml").exists() {
                    let name = path.file_name().unwrap();
                    let link_path = registry_dir.join(name);
                    if !link_path.exists() {
                        #[cfg(unix)]
                        let _ = std::os::unix::fs::symlink(&path, &link_path);
                    }
                }
            }
        }
    }

    println!("cargo:rerun-if-changed=build.rs");
}