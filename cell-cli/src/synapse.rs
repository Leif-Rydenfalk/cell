use anyhow::{anyhow, Result};
use snow::TransportState;
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

/// A wrapper holding the TCP stream and the Crypto state.
pub struct SecureStream {
    pub inner: TcpStream,
    pub state: TransportState,
}

impl SecureStream {
    pub fn get_remote_static(&self) -> Option<&[u8]> {
        self.state.get_remote_static()
    }
}

/// Performs the Noise_XX handshake manually.
pub async fn connect_secure(
    mut stream: TcpStream,
    local_keys: &snow::Keypair,
    is_initiator: bool,
) -> Result<(SecureStream, Vec<u8>)> {
    let builder = snow::Builder::new("Noise_XX_25519_ChaChaPoly_BLAKE2s".parse()?);
    let mut state = if is_initiator {
        builder
            .local_private_key(&local_keys.private)
            .build_initiator()?
    } else {
        builder
            .local_private_key(&local_keys.private)
            .build_responder()?
    };

    let mut buf = vec![0u8; 65535];

    if is_initiator {
        // -> e
        let len = state.write_message(&[], &mut buf)?;
        write_frame(&mut stream, &buf[..len]).await?;

        // <- e, ee, s, es
        let msg = read_frame(&mut stream).await?;
        state.read_message(&msg, &mut buf)?;

        // -> s, se
        let len = state.write_message(&[], &mut buf)?;
        write_frame(&mut stream, &buf[..len]).await?;
    } else {
        // <- e
        let msg = read_frame(&mut stream).await?;
        state.read_message(&msg, &mut buf)?;

        // -> e, ee, s, es
        let len = state.write_message(&[], &mut buf)?;
        write_frame(&mut stream, &buf[..len]).await?;

        // <- s, se
        let msg = read_frame(&mut stream).await?;
        state.read_message(&msg, &mut buf)?;
    }

    let transport_state = state.into_transport_mode()?;

    let remote_static = transport_state
        .get_remote_static()
        .ok_or_else(|| anyhow!("Handshake incomplete: No remote key"))?
        .to_vec();

    Ok((
        SecureStream {
            inner: stream,
            state: transport_state,
        },
        remote_static,
    ))
}

// --- Helpers ---

pub async fn write_frame(stream: &mut TcpStream, data: &[u8]) -> Result<()> {
    let len = (data.len() as u16).to_le_bytes();
    stream.write_all(&len).await?;
    stream.write_all(data).await?;
    Ok(())
}

pub async fn read_frame(stream: &mut TcpStream) -> Result<Vec<u8>> {
    let mut len_buf = [0u8; 2];
    stream.read_exact(&mut len_buf).await?;
    let len = u16::from_le_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;
    Ok(buf)
}

/// The Main Loop: Bridges Encrypted TCP <-> Plaintext Unix Socket
pub async fn bridge_secure_to_plain<P>(mut secure: SecureStream, mut plain: P) -> Result<()>
where
    P: AsyncRead + AsyncWrite + Unpin,
{
    let mut p_buf = vec![0u8; 65535];
    let mut s_buf_out = vec![0u8; 65535 + 16];

    loop {
        tokio::select! {
            // 1. Receive Encrypted Frame -> Decrypt -> Write to Plain
            res = read_frame(&mut secure.inner) => {
                let encrypted_frame = match res {
                    Ok(f) => f,
                    Err(_) => break, // Connection closed
                };

                let len = secure.state.read_message(&encrypted_frame, &mut p_buf)
                    .map_err(|_| anyhow!("Decryption failed"))?;

                plain.write_all(&p_buf[..len]).await?;
            }

            // 2. Receive Plaintext -> Encrypt -> Write Frame to Secure
            res = plain.read(&mut p_buf) => {
                let n = match res {
                    Ok(0) => break, // EOF
                    Ok(n) => n,
                    Err(_) => break,
                };

                let len = secure.state.write_message(&p_buf[..n], &mut s_buf_out)
                    .map_err(|_| anyhow!("Encryption failed"))?;

                write_frame(&mut secure.inner, &s_buf_out[..len]).await?;
            }
        }
    }
    Ok(())
}
