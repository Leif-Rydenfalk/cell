pub mod vesicle;

use anyhow::{bail, Context, Result};
pub use cell_macros::{call_as, signal_receptor};
use rkyv::AlignedVec;
use std::io::{Read, Write};
use std::os::unix::io::FromRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::Arc;
use vesicle::Vesicle;

// Re-export rkyv for macros
pub use rkyv;

/// The Membrane is the boundary of your logic.
pub struct Membrane;

impl Membrane {
    pub fn bind<F>(signal_def: &str, handler: F) -> Result<()>
    where
        F: Fn(Vesicle) -> Result<Vesicle> + Send + Sync + 'static,
    {
        let listener = if let Ok(fd_str) = std::env::var("CELL_SOCKET_FD") {
            let fd: i32 = fd_str.parse().context("Invalid CELL_SOCKET_FD")?;
            unsafe { UnixListener::from_raw_fd(fd) }
        } else {
            // Dev mode fallback
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

/// A Synapse is a connection to another Cell.
pub struct Synapse {
    stream: UnixStream,
}

impl Synapse {
    /// Grow a synapse towards a target cell name.
    /// Connects to local Golgi, performs handshake, establishes bridge.
    pub fn grow(target_cell: &str) -> Result<Self> {
        let golgi_path = std::env::var("CELL_GOLGI_SOCK")
            .context("CELL_GOLGI_SOCK not set. Are you running inside 'membrane mitosis'?")?;

        let mut stream = UnixStream::connect(&golgi_path)
            .with_context(|| format!("Failed to connect to Golgi at {}", golgi_path))?;

        // Protocol: [Op: 0x01] [Len: u32] [Name]
        stream.write_all(&[0x01])?;
        let name_bytes = target_cell.as_bytes();
        stream.write_all(&(name_bytes.len() as u32).to_be_bytes())?;
        stream.write_all(name_bytes)?;

        // Wait for Ack from Golgi
        let mut ack = [0u8; 1];
        stream.read_exact(&mut ack)?;
        if ack[0] != 0x00 {
            bail!(
                "Golgi rejected connection to '{}' (Code: {:x})",
                target_cell,
                ack[0]
            );
        }

        Ok(Self { stream })
    }

    /// Fire a signal (send a Vesicle) and await response.
    pub fn fire(&mut self, vesicle: Vesicle) -> Result<Vesicle> {
        // Send Length + Data
        self.stream
            .write_all(&(vesicle.len() as u32).to_be_bytes())?;
        self.stream.write_all(vesicle.as_slice())?;
        self.stream.flush()?;

        // Receive Response
        read_vesicle(&mut self.stream)
    }
}

fn handle_transport(
    stream: &mut UnixStream,
    genome: &[u8],
    handler: &dyn Fn(Vesicle) -> Result<Vesicle>,
) -> Result<()> {
    loop {
        // Check if stream is closed
        let incoming = match read_vesicle(stream) {
            Ok(v) => v,
            Err(_) => break,
        };

        // Genome Discovery Request
        if incoming.as_slice() == b"__GENOME__" {
            let v_out = Vesicle::wrap(genome.to_vec());
            send_vesicle(stream, v_out)?;
            continue;
        }

        // User Logic
        match handler(incoming) {
            Ok(response) => send_vesicle(stream, response)?,
            Err(e) => {
                eprintln!("Cytosol Error: {:?}", e);
                // We should probably send an error frame here in a real protocol
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

    // Sanity limit
    if len > 512 * 1024 * 1024 {
        bail!("Vesicle too large (>512MB)");
    }

    let mut v = Vesicle::with_capacity(len);
    stream.read_exact(v.as_mut_slice())?;
    Ok(v)
}

fn send_vesicle(stream: &mut UnixStream, v: Vesicle) -> Result<()> {
    stream.write_all(&(v.len() as u32).to_be_bytes())?;
    stream.write_all(v.as_slice())?;
    Ok(())
}
