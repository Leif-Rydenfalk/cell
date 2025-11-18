//! cell-sdk  –  Biological-cell RPC framework (runtime-only edition)

use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::io::FromRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

pub use cell_macros::{call_as, service_schema};

// ---------- server side ----------
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
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > 16 * 1024 * 1024 {
        bail!("message too large: {} bytes", len);
    }

    let mut msg_buf = vec![0u8; len];
    stream.read_exact(&mut msg_buf)?;

    if &msg_buf == b"__SCHEMA__" {
        let resp = schema_json.as_bytes();
        stream.write_all(&(resp.len() as u32).to_be_bytes())?;
        stream.write_all(resp)?;
        return Ok(());
    }

    let request = std::str::from_utf8(&msg_buf).context("invalid utf-8")?;
    let response_json = handler(request)?;
    let resp = response_json.as_bytes();
    stream.write_all(&(resp.len() as u32).to_be_bytes())?;
    stream.write_all(resp)?;
    stream.flush()?;
    Ok(())
}

// ---------- runtime schema fetch ----------
lazy_static::lazy_static! {
    static ref SCHEMA_CACHE: Mutex<HashMap<String, String>> = Mutex::new(HashMap::new());
}

pub fn fetch_schema_runtime(service: &str) -> Result<String> {
    // Check cache first
    {
        let cache = SCHEMA_CACHE.lock().unwrap();
        if let Some(cached) = cache.get(service) {
            return Ok(cached.clone());
        }
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

    // Cache the schema
    {
        let mut cache = SCHEMA_CACHE.lock().unwrap();
        cache.insert(service.to_string(), schema.clone());
    }

    Ok(schema)
}

// ---------- build helper (kept for compat) ----------
#[cfg(feature = "build")]
pub mod build {
    use super::*;
    use std::fs;
    use std::path::Path;

    pub fn fetch_and_cache_schema(service_name: &str, out_dir: &Path) -> Result<(), String> {
        let schema = fetch_schema_runtime(service_name).map_err(|e| e.to_string())?;

        // Write JSON schema
        let schema_path = out_dir.join(format!("{}_schema.json", service_name));
        fs::write(&schema_path, &schema).map_err(|e| format!("failed to write schema: {}", e))?;

        // Generate Rust code from schema
        let rust_code = generate_rust_from_schema(&schema)
            .map_err(|e| format!("failed to generate Rust code: {}", e))?;
        let code_path = out_dir.join(format!("{}_types.rs", service_name));
        fs::write(&code_path, rust_code).map_err(|e| format!("failed to write types: {}", e))?;

        println!(
            "cargo:warning=✓ Cached schema for '{}' (runtime fetch)",
            service_name
        );
        Ok(())
    }

    fn generate_rust_from_schema(schema_json: &str) -> Result<String, String> {
        let schema: serde_json::Value =
            serde_json::from_str(schema_json).map_err(|e| format!("invalid schema JSON: {}", e))?;

        let req_name = schema["request"]["name"]
            .as_str()
            .ok_or("missing request name")?;
        let resp_name = schema["response"]["name"]
            .as_str()
            .ok_or("missing response name")?;

        let req_fields = schema["request"]["fields"]
            .as_array()
            .ok_or("missing request fields")?;
        let resp_fields = schema["response"]["fields"]
            .as_array()
            .ok_or("missing response fields")?;

        let mut code = String::new();
        code.push_str("// Auto-generated types from schema\n\n");
        code.push_str("#[allow(dead_code)]\n");
        code.push_str("#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]\n");
        code.push_str(&format!("pub struct {} {{\n", req_name));
        for field in req_fields {
            let name = field["name"].as_str().ok_or("missing field name")?;
            let ty = field["type"].as_str().ok_or("missing field type")?;
            code.push_str(&format!("    pub {}: {},\n", name, ty));
        }
        code.push_str("}\n\n");

        code.push_str("#[allow(dead_code)]\n");
        code.push_str("#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]\n");
        code.push_str(&format!("pub struct {} {{\n", resp_name));
        for field in resp_fields {
            let name = field["name"].as_str().ok_or("missing field name")?;
            let ty = field["type"].as_str().ok_or("missing field type")?;
            code.push_str(&format!("    pub {}: {},\n", name, ty));
        }
        code.push_str("}\n");

        Ok(code)
    }
}
