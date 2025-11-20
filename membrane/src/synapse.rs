use anyhow::{anyhow, Result};
use snow::{Builder, TransportState};
use snowstorm::NoiseStream;
use tokio::net::TcpStream;

/// Performs the handshake and returns a high-performance, encrypted stream.
pub async fn connect_secure(
    mut stream: TcpStream,
    local_keys: &snow::Keypair,
    is_initiator: bool,
) -> Result<(NoiseStream<TcpStream>, Vec<u8>)> {
    
    // 1. Setup Noise State (Same as before)
    let builder = Builder::new("Noise_XX_25519_ChaChaPoly_BLAKE2s".parse()?);
    let mut state = if is_initiator {
        builder.local_private_key(&local_keys.private).build_initiator()?
    } else {
        builder.local_private_key(&local_keys.private).build_responder()?
    };

    // 2. Manual Handshake (To verify Identity)
    let mut buf = vec![0u8; 65535];
    
    // We use snowstorm's helper to drive the handshake on the TCP stream
    // This handles the length-prefixed framing automatically during handshake
    handshake_loop(&mut stream, &mut state, is_initiator, &mut buf).await?;

    // 3. Extract Remote Identity (The Public Key)
    // We need this for the "Immune System" check
    let remote_static = state.get_remote_static()
        .ok_or_else(|| anyhow!("Handshake did not yield a remote public key"))?
        .to_vec();

    // 4. Convert to Lock-Free NoiseStream
    // Snowstorm takes ownership of the state and handles the Rx/Tx splitting internally
    let secure_stream = NoiseStream::new(stream, state);

    Ok((secure_stream, remote_static))
}

/// Helper to drive the raw snow state machine over TCP before wrapping it
async fn handshake_loop(
    stream: &mut TcpStream,
    state: &mut TransportState,
    is_initiator: bool,
    buf: &mut [u8],
) -> Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    loop {
        if state.is_handshake_finished() {
            return Ok(());
        }

        if is_initiator {
            let len = state.write_message(&[], buf)?;
            write_frame(stream, &buf[..len]).await?;

            if !state.is_handshake_finished() {
                let msg = read_frame(stream).await?;
                state.read_message(&msg, buf)?;
            }
        } else {
            let msg = read_frame(stream).await?;
            state.read_message(&msg, buf)?;

            if !state.is_handshake_finished() {
                let len = state.write_message(&[], buf)?;
                write_frame(stream, &buf[..len]).await?;
            }
        }
    }
}

// Minimal framing helpers for the handshake phase only
async fn write_frame(stream: &mut TcpStream, data: &[u8]) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    let len = (data.len() as u16).to_le_bytes(); // Snowstorm uses Little Endian by default
    stream.write_all(&len).await?;
    stream.write_all(data).await?;
    Ok(())
}

async fn read_frame(stream: &mut TcpStream) -> Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let mut len_buf = [0u8; 2];
    stream.read_exact(&mut len_buf).await?;
    let len = u16::from_le_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;
    Ok(buf)
}