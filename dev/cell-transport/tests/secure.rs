#[cfg(test)]
mod tests {
    use cell_transport::transport::{SecureTransport, UnixTransport};
    use cell_core::Transport;
    use tokio::net::UnixStream;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use rand::RngCore;

    // A mock transport that acts as a loopback pipe
    struct MockTransport {
        tx: tokio::sync::mpsc::Sender<Vec<u8>>,
        rx: std::sync::Arc<tokio::sync::Mutex<tokio::sync::mpsc::Receiver<Vec<u8>>>>,
    }

    impl cell_core::Transport for MockTransport {
        fn call(&self, data: &[u8]) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>, cell_core::CellError>> + Send + '_>> {
            let data_vec = data.to_vec();
            let tx = self.tx.clone();
            let rx = self.rx.clone();
            
            Box::pin(async move {
                // Send "request"
                tx.send(data_vec).await.map_err(|_| cell_core::CellError::IoError)?;
                
                // Wait for "response"
                let mut guard = rx.lock().await;
                let resp = guard.recv().await.ok_or(cell_core::CellError::ConnectionReset)?;
                Ok(resp)
            })
        }
    }

    #[tokio::test]
    async fn test_secure_transport_encryption() {
        // Shared Key
        let mut key = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut key);

        // Channels to simulate network
        let (client_tx, mut server_rx) = tokio::sync::mpsc::channel(1);
        let (server_tx, client_rx) = tokio::sync::mpsc::channel(1);

        // Client Side
        let mock_transport = MockTransport {
            tx: client_tx,
            rx: std::sync::Arc::new(tokio::sync::Mutex::new(client_rx)),
        };
        let secure_client = SecureTransport::new(mock_transport, key);

        // Server Side Logic (Manually decrypting to verify)
        let key_clone = key;
        tokio::spawn(async move {
            let cipher = chacha20poly1305::ChaCha20Poly1305::new(
                &chacha20poly1305::Key::from_slice(&key_clone)
            );
            use chacha20poly1305::aead::{Aead, NewAead};
            
            // Hardcoded nonce from implementation (needs to match source logic)
            // In the source, we used 12 zero bytes for the MVP.
            let nonce = chacha20poly1305::Nonce::from_slice(&[0u8; 12]);

            if let Some(encrypted_req) = server_rx.recv().await {
                // 1. Verify it IS encrypted (should look random/different from plaintext)
                assert_ne!(encrypted_req, b"secret_payload");

                // 2. Decrypt
                let decrypted = cipher.decrypt(nonce, encrypted_req.as_ref())
                    .expect("Server failed to decrypt");
                
                assert_eq!(decrypted, b"secret_payload");

                // 3. Encrypt Response
                let response_plaintext = b"secure_response";
                let encrypted_resp = cipher.encrypt(nonce, response_plaintext.as_ref())
                    .expect("Server failed to encrypt response");
                
                server_tx.send(encrypted_resp).await.unwrap();
            }
        });

        // Execute Client Call
        let result = secure_client.call(b"secret_payload").await;
        
        assert!(result.is_ok());
        let response_payload = result.unwrap();
        assert_eq!(response_payload, b"secure_response");
    }
}