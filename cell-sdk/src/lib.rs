use std::os::unix::net::UnixListener;
use std::io::{Read, Write};
use anyhow::Result;

// Re-export macros
pub use cell_macros::{service_schema, call_as};

/// Run a service with schema introspection
pub fn run_service_with_schema<F>(
    service_name: &str,
    schema_json: &str,
    handler: F,
) -> Result<()>
where
    F: Fn(&str) -> Result<String>,
{
    let socket_path = std::env::var("CELL_SOCKET_PATH")
        .unwrap_or_else(|_| format!("/tmp/cell/sockets/{}.sock", service_name));
    
    let _ = std::fs::remove_file(&socket_path);
    
    println!("🚀 Service '{}' starting", service_name);
    println!("   Socket: {}", socket_path);
    
    let listener = UnixListener::bind(&socket_path)?;
    println!("✓  Service ready");
    
    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                // Read length-prefixed message
                let mut len_buf = [0u8; 4];
                match stream.read_exact(&mut len_buf) {
                    Ok(_) => {
                        let len = u32::from_be_bytes(len_buf) as usize;
                        
                        // Sanity check (max 16MB)
                        if len > 16 * 1024 * 1024 {
                            eprintln!("❌ Message too large: {} bytes", len);
                            continue;
                        }
                        
                        let mut msg_buf = vec![0u8; len];
                        if let Err(e) = stream.read_exact(&mut msg_buf) {
                            eprintln!("❌ Read error: {}", e);
                            continue;
                        }
                        
                        // Schema introspection
                        if &msg_buf == b"__SCHEMA__" {
                            let response = schema_json.as_bytes();
                            stream.write_all(&(response.len() as u32).to_be_bytes())?;
                            stream.write_all(response)?;
                            stream.flush()?;
                            continue;
                        }
                        
                        // Normal request
                        let request_json = std::str::from_utf8(&msg_buf).unwrap_or("");
                        match handler(request_json) {
                            Ok(response_json) => {
                                let response = response_json.as_bytes();
                                stream.write_all(&(response.len() as u32).to_be_bytes())?;
                                stream.write_all(response)?;
                                stream.flush()?;
                            }
                            Err(e) => {
                                eprintln!("❌ Handler error: {}", e);
                            }
                        }
                    }
                    Err(_) => {
                        // Connection closed or error
                        break;
                    }
                }
            }
            Err(e) => eprintln!("❌ Accept error: {}", e),
        }
    }
    
    Ok(())
}

// ============================================
// Build-time helpers (for build.rs)
// ============================================

#[cfg(feature = "build")]
pub mod build {
    use std::os::unix::net::UnixStream;
    use std::io::{Read, Write};
    use std::path::Path;
    use std::fs;
    use std::time::Duration;
    
    /// Fetch schema from running service and cache it
    pub fn fetch_and_cache_schema(service_name: &str, out_dir: &Path) -> Result<(), String> {
        let socket_path = format!("/tmp/cell/sockets/{}.sock", service_name);
        
        let mut stream = UnixStream::connect(&socket_path)
            .map_err(|e| format!("Service '{}' not running: {}", service_name, e))?;
        
        stream.set_read_timeout(Some(Duration::from_secs(2)))
            .map_err(|e| format!("Timeout error: {}", e))?;
        
        // Send length-prefixed __SCHEMA__ request
        let request = b"__SCHEMA__";
        stream.write_all(&(request.len() as u32).to_be_bytes())
            .map_err(|e| format!("Write error: {}", e))?;
        stream.write_all(request)
            .map_err(|e| format!("Write error: {}", e))?;
        stream.flush()
            .map_err(|e| format!("Flush error: {}", e))?;
        
        // Read length-prefixed response
        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf)
            .map_err(|e| format!("Read length error: {}", e))?;
        
        let len = u32::from_be_bytes(len_buf) as usize;
        let mut schema_buf = vec![0u8; len];
        stream.read_exact(&mut schema_buf)
            .map_err(|e| format!("Read schema error: {}", e))?;
        
        let schema_hash = blake3::hash(&schema_buf).to_hex().to_string();
        
        // Write schema to OUT_DIR
        let schema_path = out_dir.join(format!("{}_schema.json", service_name));
        fs::write(&schema_path, &schema_buf)
            .map_err(|e| format!("Failed to write schema: {}", e))?;
        
        // Write hash
        let hash_path = out_dir.join(format!("{}_hash.txt", service_name));
        fs::write(&hash_path, schema_hash.as_bytes())
            .map_err(|e| format!("Failed to write hash: {}", e))?;
        
        println!("cargo:warning=✓ Cached schema for '{}' (hash: {})", service_name, &schema_hash[..16]);
        println!("cargo:rerun-if-changed={}", socket_path);
        
        Ok(())
    }
}
