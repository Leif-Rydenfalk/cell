//! cell-sdk  –  Biological-cell RPC framework (runtime-only version)
//! https://github.com/Leif-Rydenfalk/cell

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

pub use cell_macros::{call_as, service_schema};

// ------------------------------------------------------------------
// 1.  Server side – unchanged from 0.1.1
// ------------------------------------------------------------------

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

// ------------------------------------------------------------------
// 2.  Runtime-only schema fetch (replaces OUT_DIR requirement)
// ------------------------------------------------------------------

static SCHEMA_CACHE: OnceLock<std::collections::HashMap<String, String>> = OnceLock::new();

/// Fetch schema at runtime (with memoisation).
pub fn fetch_schema_runtime(service: &str) -> Result<String> {
    let cache = SCHEMA_CACHE.get_or_init(Default::default);
    if let Some(cached) = cache.get(service) {
        return Ok(cached.clone());
    }

    let sock_path = format!("/tmp/cell/sockets/{}.sock", service);
    let mut s = UnixStream::connect(&sock_path)
        .with_context(|| format!("service '{}' not running", service))?;
    s.set_read_timeout(Some(Duration::from_secs(2)))?;

    let req = b"__SCHEMA__";
    s.write_all(&(req.len() as u32).to_be_bytes())?;
    s.write_all(req)?;
    s.flush()?;

    let mut len_buf = [0u8; 4];
    s.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut schema_bytes = vec![0u8; len];
    s.read_exact(&mut schema_bytes)?;

    let schema = String::from_utf8_lossy(&schema_bytes).into_owned();
    cache.insert(service.to_string(), schema.clone());
    Ok(schema)
}

// ------------------------------------------------------------------
// 3.  Optional build-time helper (kept for backward compat)
// ------------------------------------------------------------------
#[cfg(feature = "build")]
pub mod build {
    use super::*;
    use std::fs;
    use std::path::Path;

    /// Fetch schema and cache it in `out_dir`.
    pub fn fetch_and_cache_schema(service_name: &str, out_dir: &Path) -> Result<(), String> {
        let schema = fetch_schema_runtime(service_name).map_err(|e| e.to_string())?;
        let schema_path = out_dir.join(format!("{}_schema.json", service_name));
        fs::write(&schema_path, &schema).map_err(|e| format!("failed to write schema: {}", e))?;
        println!(
            "cargo:warning=✓ Cached schema for '{}' (runtime fetch)",
            service_name
        );
        Ok(())
    }

    /// Tiny helper to be called from build.rs when users *want* build-time caching.
    pub fn run_build_script() -> Result<()> {
        let out_dir = std::env::var("OUT_DIR").map_err(|_| "OUT_DIR not set")?;
        let out_path = Path::new(&out_dir);
        for dep in &["worker", "aggregator"] {
            // parse cell.toml in real life; here we keep it short
            let _ = fetch_and_cache_schema(dep, out_path);
        }
        Ok(())
    }
}
