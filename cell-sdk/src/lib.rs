// pub mod quic;
pub mod vesicle;

use anyhow::{bail, Context, Result};
// Re-export macros, including the new #[protein]
pub use cell_macros::{call_as, protein, signal_receptor};
// Re-export rkyv so the macros can find it reliably via ::cell_sdk::rkyv
pub use rkyv;

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::io::FromRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::Arc;
use vesicle::Vesicle;

// Thread-local pool to reuse connections (Keep-Alive)
thread_local! {
    static CONNECTION_POOL: RefCell<HashMap<String, UnixStream>> = RefCell::new(HashMap::new());
}

pub struct Membrane;

impl Membrane {
    pub fn bind<F>(signal_def: &str, handler: F) -> Result<()>
    where
        F: Fn(Vesicle) -> Result<Vesicle> + Send + Sync + 'static,
    {
        // 1. Socket Acquisition
        // Try to inherit FD from Daemon (Production Mode)
        let listener = if let Ok(fd_str) = std::env::var("CELL_SOCKET_FD") {
            let fd: i32 = fd_str.parse().context("Invalid CELL_SOCKET_FD")?;
            unsafe { UnixListener::from_raw_fd(fd) }
        } else {
            // Fallback: Bind manually (Dev/Test Mode)
            let path = Path::new("run/cell.sock");
            if let Some(p) = path.parent() {
                std::fs::create_dir_all(p)?;
            }
            if path.exists() {
                std::fs::remove_file(path)?;
            }
            UnixListener::bind(path)?
        };

        let genome_trait = signal_def.as_bytes().to_vec();
        let handler = Arc::new(handler);

        // 2. Main Loop
        // We use a threaded model here for the SDK to keep the API simple/blocking
        // for the user, while the Daemon handles async IO.
        for stream in listener.incoming() {
            match stream {
                Ok(mut s) => {
                    let h = handler.clone();
                    let g = genome_trait.clone();
                    std::thread::spawn(move || {
                        let _ = handle_transport(&mut s, &g, &*h);
                    });
                }
                Err(_) => {}
            }
        }
        Ok(())
    }
}

pub struct Synapse {
    stream: UnixStream,
    target: String,
}

impl Synapse {
    pub fn grow(target_cell: &str) -> Result<Self> {
        // 1. Check Pool
        let cached_stream = CONNECTION_POOL.with(|pool| pool.borrow_mut().remove(target_cell));

        if let Some(stream) = cached_stream {
            return Ok(Self {
                stream,
                target: target_cell.to_string(),
            });
        }

        // 2. Connect New
        let golgi_path =
            std::env::var("CELL_GOLGI_SOCK").unwrap_or_else(|_| "run/golgi.sock".to_string());

        let mut stream = UnixStream::connect(&golgi_path)
            .with_context(|| format!("Failed to connect to Golgi at {}", golgi_path))?;

        // 3. Handshake: [Op: 0x01] [Len: u32] [Name]
        stream.write_all(&[0x01])?;
        let name_bytes = target_cell.as_bytes();
        stream.write_all(&(name_bytes.len() as u32).to_be_bytes())?;
        stream.write_all(name_bytes)?;

        // 4. Wait for Ack
        let mut ack = [0u8; 1];
        stream.read_exact(&mut ack)?;
        if ack[0] != 0x00 {
            bail!(
                "Golgi rejected connection to '{}' (Code: {:x})",
                target_cell,
                ack[0]
            );
        }

        Ok(Self {
            stream,
            target: target_cell.to_string(),
        })
    }

    pub fn fire(mut self, vesicle: Vesicle) -> Result<Vesicle> {
        // Send Request
        if let Err(e) = self.write_vesicle(&vesicle) {
            // If write fails, discard stream (don't pool it)
            return Err(e);
        }

        // Receive Response
        let response = match read_vesicle(&mut self.stream) {
            Ok(v) => v,
            Err(e) => return Err(e),
        };

        // Recycle Stream to Pool
        CONNECTION_POOL.with(|pool| {
            pool.borrow_mut().insert(self.target.clone(), self.stream);
        });

        Ok(response)
    }

    fn write_vesicle(&mut self, v: &Vesicle) -> Result<()> {
        self.stream.write_all(&(v.len() as u32).to_be_bytes())?;
        self.stream.write_all(v.as_slice())?;
        self.stream.flush()?;
        Ok(())
    }
}

// Internal Protocol Handler
fn handle_transport(
    stream: &mut UnixStream,
    genome: &[u8],
    handler: &dyn Fn(Vesicle) -> Result<Vesicle>,
) -> Result<()> {
    loop {
        // 1. Read the packet
        let incoming = match read_vesicle(stream) {
            Ok(v) => v,
            Err(_) => break, // Clean EOF (Client disconnected)
        };

        // 2. Handle Schema Sync (Internal Signal from Golgi)
        if incoming.as_slice() == b"__GENOME__" {
            let v_out = Vesicle::wrap(genome.to_vec());
            if let Err(e) = send_vesicle(stream, v_out) {
                eprintln!("[SDK] Failed to send Genome: {}", e);
                break;
            }
            continue;
        }

        // 3. Invoke User Logic
        match handler(incoming) {
            Ok(response) => {
                if let Err(e) = send_vesicle(stream, response) {
                    eprintln!("[SDK] Failed to send Response: {}", e);
                    break;
                }
            }
            Err(e) => {
                // Log error to stderr (captured by Nucleus logger) and drop connection
                eprintln!("[SDK] Handler Error (Dropping Connection): {:?}", e);
                break;
            }
        }
    }
    Ok(())
}

fn read_vesicle(stream: &mut UnixStream) -> Result<Vesicle> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;

    // Optimization: In the future, we can use a buffer pool here.
    let mut v = Vesicle::with_capacity(len);
    stream.read_exact(v.as_mut_slice())?;
    Ok(v)
}

fn send_vesicle(stream: &mut UnixStream, v: Vesicle) -> Result<()> {
    stream.write_all(&(v.len() as u32).to_be_bytes())?;
    stream.write_all(v.as_slice())?;
    Ok(())
}
