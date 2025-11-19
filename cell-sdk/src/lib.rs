use anyhow::{bail, Context, Result};
use std::io::{Read, Write};
use std::os::unix::io::FromRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

// Export macros
pub use cell_macros::{call_as, service_schema};
pub use rkyv;

// --- CLIENT ---

pub struct CellClient {
    stream: UnixStream,
    wbuf: Vec<u8>,
    batch_limit: usize,
    pending_count: usize,
}

impl CellClient {
    /// Connects to the local Cell Router and requests a specific service
    pub fn connect(service_name: &str) -> Result<Self> {
        Self::connect_with_batch(service_name, 1)
    }

    pub fn connect_with_batch(service_name: &str, batch_size: usize) -> Result<Self> {
        // 1. Find the Router Socket (injected by the CLI)
        let router_path = std::env::var("CELL_ROUTER_SOCK")
            .context("CELL_ROUTER_SOCK not set. Are you running inside 'cell run'?")?;

        let mut stream = UnixStream::connect(&router_path)
            .with_context(|| format!("Failed to connect to router at {}", router_path))?;

        // 2. Router Handshake
        // Protocol: [OpCode: 1] [Name Len: 4] [Name Bytes]
        stream.write_all(&[0x01])?;
        let name_bytes = service_name.as_bytes();
        stream.write_all(&(name_bytes.len() as u32).to_be_bytes())?;
        stream.write_all(name_bytes)?;

        // 3. Wait for Ack (0x00 = OK)
        let mut ack = [0u8; 1];
        stream.read_exact(&mut ack)?;
        if ack[0] != 0x00 {
            bail!(
                "Router refused connection to '{}' (Code: {:x})",
                service_name,
                ack[0]
            );
        }

        stream.set_nonblocking(false)?;
        // stream.set_read_timeout(Some(std::time::Duration::from_secs(60)))?;

        Ok(Self {
            stream,
            wbuf: Vec::with_capacity(4096 * batch_size),
            batch_limit: batch_size,
            pending_count: 0,
        })
    }

    pub fn call(&mut self, payload: &[u8]) -> Result<Vec<u8>> {
        if self.batch_limit == 1 {
            self.stream
                .write_all(&(payload.len() as u32).to_be_bytes())?;
            self.stream.write_all(payload)?;
            read_response(&mut self.stream).map_err(|e| e.into())
        } else {
            // In batched mode, call() queues and returns empty. User must manually flush/read.
            Ok(Vec::new())
        }
    }

    pub fn queue_request(&mut self, payload: &[u8]) -> Result<bool> {
        self.wbuf
            .extend_from_slice(&(payload.len() as u32).to_be_bytes());
        self.wbuf.extend_from_slice(payload);
        self.pending_count += 1;

        if self.pending_count >= self.batch_limit {
            self.stream.write_all(&self.wbuf)?;
            self.wbuf.clear();
            self.pending_count = 0;
            return Ok(true);
        }
        Ok(false)
    }

    pub fn read_n_responses(&mut self, n: usize) -> Result<()> {
        for _ in 0..n {
            let _ = read_response(&mut self.stream)?;
        }
        Ok(())
    }

    pub fn flush_writes(&mut self) -> Result<()> {
        if !self.wbuf.is_empty() {
            self.stream.write_all(&self.wbuf)?;
            self.wbuf.clear();
            self.pending_count = 0;
        }
        Ok(())
    }
}

pub fn invoke_rpc(service_name: &str, payload: &[u8]) -> Result<Vec<u8>> {
    let mut client = CellClient::connect(service_name)?;
    client.call(payload)
}

fn read_response(stream: &mut UnixStream) -> std::io::Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf)?;
    Ok(buf)
}

// --- SERVER ---

pub fn run_service_with_schema<F>(service_name: &str, schema_json: &str, handler: F) -> Result<()>
where
    F: Fn(&[u8]) -> Result<Vec<u8>> + Send + Sync + 'static,
{
    // Socket Activation Logic
    let listener = if let Ok(fd_str) = std::env::var("CELL_SOCKET_FD") {
        let fd: i32 = fd_str.parse().context("CELL_SOCKET_FD invalid")?;
        unsafe { UnixListener::from_raw_fd(fd) }
    } else {
        // Fallback for local testing without CLI
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

    listener.set_nonblocking(false)?;
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
                let h = handler_arc.clone();
                let schema = schema_bytes.clone();
                std::thread::spawn(move || {
                    if let Err(e) = handle_client_loop(&mut s, &schema, &*h) {
                        // Don't spam logs on disconnect
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
    schema: &[u8],
    handler: &dyn Fn(&[u8]) -> Result<Vec<u8>>,
) -> anyhow::Result<()> {
    loop {
        let mut len_buf = [0u8; 4];
        if let Err(_) = stream.read_exact(&mut len_buf) {
            return Err(anyhow::anyhow!("Client disconnected"));
        }
        let len = u32::from_be_bytes(len_buf) as usize;
        // Safety limit: 256MB
        if len > 256 * 1024 * 1024 {
            bail!("Message too large");
        }

        let mut msg_buf = vec![0u8; len];
        stream.read_exact(&mut msg_buf)?;

        if &msg_buf == b"__SCHEMA__" {
            stream.write_all(&(schema.len() as u32).to_be_bytes())?;
            stream.write_all(schema)?;
            continue;
        }

        let resp = handler(&msg_buf)?;
        stream.write_all(&(resp.len() as u32).to_be_bytes())?;
        stream.write_all(&resp)?;
    }
}
