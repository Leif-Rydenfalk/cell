pub use cell_macros::{call_as, service_schema};

use anyhow::bail;
use anyhow::Context;
use anyhow::Result;
use std::io::{Read, Write};
use std::os::unix::io::FromRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::time::SystemTime;

/// Run a service with schema introspection.
/// If CELL_SOCKET_FD is present we use the inherited listener, otherwise we bind ourselves.
pub fn run_service_with_schema<F>(service_name: &str, schema_json: &str, handler: F) -> Result<()>
where
    F: Fn(&str) -> Result<String> + Send + Sync + 'static,
{
    let listener = if let Ok(fd_str) = std::env::var("CELL_SOCKET_FD") {
        let fd: i32 = fd_str.parse().context("CELL_SOCKET_FD must be numeric")?;
        unsafe { UnixListener::from_raw_fd(fd) }
    } else {
        let path = std::env::var("CELL_SOCKET_PATH")
            .unwrap_or_else(|_| format!("/tmp/cell/sockets/{}.sock", service_name));
        let _ = std::fs::remove_file(&path);
        UnixListener::bind(&path).with_context(|| format!("bind {}", path))?
    };

    listener.set_nonblocking(false).context("set_nonblocking")?;

    eprintln!(
        "{} 🚀 Service '{}' ready on {:?}",
        humantime::format_rfc3339(SystemTime::now()),
        service_name,
        listener.local_addr().ok()
    );

    for stream in listener.incoming() {
        match stream {
            Ok(mut s) => {
                if let Err(e) = handle_one_client(&mut s, schema_json, &handler) {
                    eprintln!(
                        "{} ❌ Handler error: {}",
                        humantime::format_rfc3339(SystemTime::now()),
                        e
                    );
                }
            }
            Err(e) => eprintln!(
                "{} ❌ Accept error: {}",
                humantime::format_rfc3339(SystemTime::now()),
                e
            ),
        }
    }
    Ok(())
}

fn handle_one_client(
    stream: &mut UnixStream,
    schema_json: &str,
    handler: &dyn Fn(&str) -> Result<String>,
) -> Result<()> {
    // read 4-byte length header
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > 16 * 1024 * 1024 {
        bail!("message too large: {} bytes", len);
    }

    let mut msg_buf = vec![0u8; len];
    stream.read_exact(&mut msg_buf)?;

    // schema introspection
    if &msg_buf == b"__SCHEMA__" {
        let resp = schema_json.as_bytes();
        stream.write_all(&(resp.len() as u32).to_be_bytes())?;
        stream.write_all(resp)?;
        return Ok(());
    }

    // normal request
    let request = std::str::from_utf8(&msg_buf).context("invalid utf-8")?;
    let response_json = handler(request)?;
    let resp = response_json.as_bytes();
    stream.write_all(&(resp.len() as u32).to_be_bytes())?;
    stream.write_all(resp)?;
    stream.flush()?;
    Ok(())
}

// ============================================
// Build-time helpers (for build.rs)
// ============================================

#[cfg(feature = "build")]
pub mod build {
    use std::fs;
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::path::Path;
    use std::time::Duration;

    /// Fetch schema from running service and cache it
    pub fn fetch_and_cache_schema(service_name: &str, out_dir: &Path) -> Result<(), String> {
        let socket_path = format!("/tmp/cell/sockets/{}.sock", service_name);

        let mut stream = UnixStream::connect(&socket_path)
            .map_err(|e| format!("Service '{}' not running: {}", service_name, e))?;

        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .map_err(|e| format!("Timeout error: {}", e))?;

        // Send length-prefixed __SCHEMA__ request
        let request = b"__SCHEMA__";
        stream
            .write_all(&(request.len() as u32).to_be_bytes())
            .map_err(|e| format!("Write error: {}", e))?;
        stream
            .write_all(request)
            .map_err(|e| format!("Write error: {}", e))?;
        stream.flush().map_err(|e| format!("Flush error: {}", e))?;

        // Read length-prefixed response
        let mut len_buf = [0u8; 4];
        stream
            .read_exact(&mut len_buf)
            .map_err(|e| format!("Read length error: {}", e))?;

        let len = u32::from_be_bytes(len_buf) as usize;
        let mut schema_buf = vec![0u8; len];
        stream
            .read_exact(&mut schema_buf)
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

        println!(
            "cargo:warning=✓ Cached schema for '{}' (hash: {})",
            service_name,
            &schema_hash[..16]
        );
        println!("cargo:rerun-if-changed={}", socket_path);

        Ok(())
    }
}
