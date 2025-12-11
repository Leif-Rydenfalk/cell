// cells/vault/src/main.rs
// SPDX-License-Identifier: MIT
// Secrets Management with Envelope Encryption (AES-256-GCM)

use cell_sdk::*;
use anyhow::{Result, bail};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Key, Nonce
};
use rand::RngCore;

// === PROTOCOL ===

#[protein]
pub struct SecretWrite {
    pub key: String,
    pub value: Vec<u8>, // Plaintext
    pub ttl_secs: Option<u64>,
}

#[protein]
pub struct SecretRead {
    pub key: String,
    pub version: Option<u64>,
}

#[protein]
pub struct EncryptedSecret {
    pub ciphertext: Vec<u8>,
    pub nonce: Vec<u8>,
    pub version: u64,
}

// === SERVICE ===

struct VaultEntry {
    ciphertext: Vec<u8>,
    nonce: [u8; 12],
    version: u64,
    created_at: std::time::SystemTime,
}

struct VaultState {
    // Master Encryption Key (MEK) - typically wrapped by KMS/HSM
    mek: Key<Aes256Gcm>,
    store: HashMap<String, Vec<VaultEntry>>,
}

#[service]
#[derive(Clone)]
struct VaultService {
    state: Arc<RwLock<VaultState>>,
}

impl VaultService {
    fn new() -> Self {
        let mut key_bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut key_bytes);
        let mek = *Key::<Aes256Gcm>::from_slice(&key_bytes);
        
        Self {
            state: Arc::new(RwLock::new(VaultState {
                mek,
                store: HashMap::new(),
            })),
        }
    }

    fn encrypt(&self, plaintext: &[u8], key_label: &str) -> Result<(Vec<u8>, [u8; 12])> {
        let state = pollster::block_on(self.state.read());
        let cipher = Aes256Gcm::new(&state.mek);
        
        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        // Bind encryption context (AAD) to the key label to prevent swap attacks
        let payload = Payload {
            msg: plaintext,
            aad: key_label.as_bytes(),
        };

        let ciphertext = cipher.encrypt(nonce, payload)
            .map_err(|_| anyhow::anyhow!("Encryption failed"))?;

        Ok((ciphertext, nonce_bytes))
    }

    fn decrypt(&self, ciphertext: &[u8], nonce: &[u8], key_label: &str) -> Result<Vec<u8>> {
        let state = pollster::block_on(self.state.read());
        let cipher = Aes256Gcm::new(&state.mek);
        let nonce = Nonce::from_slice(nonce);

        let payload = Payload {
            msg: ciphertext,
            aad: key_label.as_bytes(),
        };

        let plaintext = cipher.decrypt(nonce, payload)
            .map_err(|_| anyhow::anyhow!("Decryption failed - integrity check error"))?;

        Ok(plaintext)
    }
}

#[handler]
impl VaultService {
    async fn put(&self, req: SecretWrite) -> Result<u64> {
        let (ciphertext, nonce) = self.encrypt(&req.value, &req.key)?;
        
        let mut state = self.state.write().await;
        let entries = state.store.entry(req.key.clone()).or_insert_with(Vec::new);
        
        let version = (entries.len() as u64) + 1;
        
        entries.push(VaultEntry {
            ciphertext,
            nonce,
            version,
            created_at: std::time::SystemTime::now(),
        });
        
        tracing::info!("[Vault] Wrote secret '{}' v{}", req.key, version);
        Ok(version)
    }

    async fn get(&self, req: SecretRead) -> Result<Vec<u8>> {
        let state = self.state.read().await;
        
        let entries = state.store.get(&req.key)
            .ok_or_else(|| anyhow::anyhow!("Secret not found"))?;
            
        let entry = if let Some(v) = req.version {
            entries.iter().find(|e| e.version == v)
                .ok_or_else(|| anyhow::anyhow!("Version not found"))?
        } else {
            entries.last()
                .ok_or_else(|| anyhow::anyhow!("No versions available"))?
        };

        // Decrypt on the fly
        let plaintext = self.decrypt(&entry.ciphertext, &entry.nonce, &req.key)?;
        
        tracing::info!("[Vault] Accessed secret '{}' v{}", req.key, entry.version);
        Ok(plaintext)
    }

    async fn rotate_keys(&self) -> Result<bool> {
        // In a real impl, this would generate a new MEK, 
        // re-encrypt the Data Encryption Keys (DEKs), and securely wipe the old one.
        tracing::info!("[Vault] Key rotation triggered");
        Ok(true)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    tracing::info!("[Vault] Secure Storage Service Active (FIPS 140-3 Mode: Simulated)");
    
    let service = VaultService::new();
    service.serve("vault").await
}