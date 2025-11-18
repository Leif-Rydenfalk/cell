//! cell-sdk  –  Biological-cell RPC framework (runtime-only edition)

use anyhow::{bail, Context, Result};
use std::io::{Read, Write};
use std::os::unix::io::FromRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::SystemTime;

pub use cell_macros::{call_as, service_schema};

/// Run a service that responds to requests matching the provided schema.
///
/// This function blocks indefinitely.
pub fn run_service_with_schema<F>(service_name: &str, schema_json: &str, handler: F) -> Result<()>
where
    F: Fn(&str) -> Result<String> + Send + Sync + 'static,
{
    // 1. Determine Listener Source
    // If CELL_SOCKET_FD is set, we are running under the 'cell' CLI/Nucleus.
    let listener = if let Ok(fd_str) = std::env::var("CELL_SOCKET_FD") {
        let fd: i32 = fd_str
            .parse()
            .context("CELL_SOCKET_FD environment variable must be an integer")?;
        unsafe { UnixListener::from_raw_fd(fd) }
    } else {
        // Standalone mode (debugging): Bind to local run/cell.sock
        let path_str =
            std::env::var("CELL_SOCKET_PATH").unwrap_or_else(|_| "run/cell.sock".to_string());
        let path = Path::new(&path_str);

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).context("Failed to create socket directory")?;
        }
        // Clean up old socket
        if path.exists() {
            std::fs::remove_file(path).context("Failed to remove existing socket")?;
        }

        UnixListener::bind(path)
            .with_context(|| format!("Failed to bind socket at {}", path.display()))?
    };

    // Ensure non-blocking is FALSE (we use blocking I/O for simplicity in the MVP)
    listener
        .set_nonblocking(false)
        .context("Failed to set socket to blocking mode")?;

    eprintln!(
        "{} 🚀 Service '{}' ready and listening",
        humantime::format_rfc3339(SystemTime::now()),
        service_name
    );

    // 2. Accept Loop
    for stream in listener.incoming() {
        match stream {
            Ok(mut s) => {
                // Handle sequentially for now. In a real high-perf scenario,
                // you would spawn a thread or task here.
                if let Err(e) = handle_one_client(&mut s, schema_json, &handler) {
                    eprintln!(
                        "{} ❌ Handler error: {:#}",
                        humantime::format_rfc3339(SystemTime::now()),
                        e
                    );
                }
            }
            Err(e) => {
                eprintln!(
                    "{} ❌ Accept error: {}",
                    humantime::format_rfc3339(SystemTime::now()),
                    e
                );
                // Don't crash the service on accept errors (like EMFILE)
            }
        }
    }
    Ok(())
}

fn handle_one_client(
    stream: &mut UnixStream,
    schema_json: &str,
    handler: &dyn Fn(&str) -> Result<String>,
) -> Result<()> {
    // Use a reasonable timeout to prevent deadlocks
    stream.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(10)))?;

    // 1. Read Request Length (4 bytes, Big Endian)
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;

    // Safety limit: 16MB max payload
    if len > 16 * 1024 * 1024 {
        bail!("Message too large: {} bytes (limit 16MB)", len);
    }

    // 2. Read Request Payload
    let mut msg_buf = vec![0u8; len];
    stream.read_exact(&mut msg_buf)?;

    // 3. Handle Schema Introspection
    if &msg_buf == b"__SCHEMA__" {
        let resp = schema_json.as_bytes();
        stream.write_all(&(resp.len() as u32).to_be_bytes())?;
        stream.write_all(resp)?;
        return Ok(());
    }

    // 4. Decode & Handle User Request
    let request = std::str::from_utf8(&msg_buf).context("Request is not valid UTF-8")?;

    // Invoke user handler
    let response_json = handler(request)?;

    // 5. Write Response
    let resp_bytes = response_json.as_bytes();
    stream.write_all(&(resp_bytes.len() as u32).to_be_bytes())?;
    stream.write_all(resp_bytes)?;
    stream.flush()?;

    Ok(())
}

// ---------- RUNTIME SCHEMA FETCHING ----------
// Used if people want to build dynamic clients manually

pub fn fetch_schema_runtime(sock_path: &str) -> Result<String> {
    let mut s = UnixStream::connect(sock_path)
        .with_context(|| format!("Failed to connect to {}", sock_path))?;

    s.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;

    let req = b"__SCHEMA__";
    s.write_all(&(req.len() as u32).to_be_bytes())?;
    s.write_all(req)?;
    s.flush()?;

    let mut len_buf = [0u8; 4];
    s.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;

    let mut schema_bytes = vec![0u8; len];
    s.read_exact(&mut schema_bytes)?;

    String::from_utf8_lossy(&schema_bytes)
        .parse()
        .map_err(|e| anyhow::anyhow!("{:?}", e))
}
