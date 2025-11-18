//! cell-sdk – Biological-cell RPC framework (Bincode Binary Protocol)

use anyhow::{bail, Context, Result};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::io::FromRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

pub use cell_macros::{call_as, service_schema};

thread_local! {
    static CONNECTION_POOL: RefCell<HashMap<String, UnixStream>> = RefCell::new(HashMap::new());
}

pub fn invoke_rpc(_service_name: &str, socket_path: &str, payload: &[u8]) -> Result<Vec<u8>> {
    let response = CONNECTION_POOL.with(|pool_cell| {
        let mut pool = pool_cell.borrow_mut();

        if let Some(mut stream) = pool.remove(socket_path) {
            if send_request(&mut stream, payload).is_ok() {
                if let Ok(resp) = read_response(&mut stream) {
                    pool.insert(socket_path.to_string(), stream);
                    return Some(Ok(resp));
                }
            }
        }
        None
    });

    if let Some(res) = response {
        return res;
    }

    let mut stream = connect_new(socket_path)?;
    send_request(&mut stream, payload)?;
    let resp = read_response(&mut stream)?;

    CONNECTION_POOL.with(|pool_cell| {
        pool_cell
            .borrow_mut()
            .insert(socket_path.to_string(), stream);
    });

    Ok(resp)
}

fn connect_new(path: &str) -> Result<UnixStream> {
    let stream = UnixStream::connect(path).with_context(|| format!("Connect to {}", path))?;
    stream
        .set_nonblocking(false)
        .context("Failed to set blocking mode")?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(30)))?;
    Ok(stream)
}

fn send_request(stream: &mut UnixStream, payload: &[u8]) -> std::io::Result<()> {
    stream.write_all(&(payload.len() as u32).to_be_bytes())?;
    stream.write_all(payload)?;
    stream.flush()
}

fn read_response(stream: &mut UnixStream) -> std::io::Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf)?;
    Ok(buf)
}

pub fn run_service_with_schema<F>(service_name: &str, schema_json: &str, handler: F) -> Result<()>
where
    F: Fn(&[u8]) -> Result<Vec<u8>> + Send + Sync + 'static,
{
    let listener = if let Ok(fd_str) = std::env::var("CELL_SOCKET_FD") {
        let fd: i32 = fd_str.parse().context("CELL_SOCKET_FD invalid")?;
        unsafe { UnixListener::from_raw_fd(fd) }
    } else {
        let path_str =
            std::env::var("CELL_SOCKET_PATH").unwrap_or_else(|_| "run/cell.sock".to_string());
        let path = Path::new(&path_str);
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        UnixListener::bind(path)?
    };

    listener
        .set_nonblocking(false)
        .context("Set listener blocking failed")?;

    eprintln!(
        "{} 🚀 Service '{}' ready",
        humantime::format_rfc3339(SystemTime::now()),
        service_name
    );

    let handler_arc = Arc::new(handler);
    let schema_bytes = schema_json.as_bytes().to_vec();

    for stream in listener.incoming() {
        match stream {
            Ok(mut s) => {
                let _ = s.set_nonblocking(false);

                let h = handler_arc.clone();
                let schema = schema_bytes.clone();

                std::thread::spawn(move || {
                    if let Err(e) = handle_client_loop(&mut s, &schema, &*h) {
                        if e.to_string() != "Client disconnected" {
                            eprintln!("Handler error: {}", e);
                        }
                    }
                });
            }
            Err(e) => eprintln!("Accept error: {}", e),
        }
    }
    Ok(())
}

fn handle_client_loop(
    stream: &mut UnixStream,
    schema_bytes: &[u8],
    handler: &dyn Fn(&[u8]) -> Result<Vec<u8>>,
) -> Result<()> {
    loop {
        let mut len_buf = [0u8; 4];
        match stream.read_exact(&mut len_buf) {
            Ok(_) => {}
            Err(ref e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Err(anyhow::anyhow!("Client disconnected"))
            }
            Err(e) => return Err(e.into()),
        }

        let len = u32::from_be_bytes(len_buf) as usize;
        if len > 256 * 1024 * 1024 {
            bail!("Message too large");
        }

        let mut msg_buf = vec![0u8; len];
        stream.read_exact(&mut msg_buf)?;

        if &msg_buf == b"__SCHEMA__" {
            stream.write_all(&(schema_bytes.len() as u32).to_be_bytes())?;
            stream.write_all(schema_bytes)?;
            stream.flush()?;
            continue;
        }

        let response_bytes = handler(&msg_buf)?;

        stream.write_all(&(response_bytes.len() as u32).to_be_bytes())?;
        stream.write_all(&response_bytes)?;
        stream.flush()?;
    }
}
