// cells/iam/src/main.rs
// SPDX-License-Identifier: MIT
// Identity & Access Management (RBAC + JWT)

use cell_sdk::*;
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use jsonwebtoken::{encode, decode, Header, Algorithm, Validation, EncodingKey, DecodingKey};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;

// === PROTOCOL ===

#[protein]
pub struct LoginRequest {
    pub client_id: String,
    pub client_secret: String, // Or proof of mTLS ownership
}

#[protein]
pub struct AuthResponse {
    pub token: String,
    pub expires_in: u64,
}

#[protein]
pub struct CheckPermission {
    pub token: String,
    pub resource: String,
    pub action: String,
}

// === JWT CLAIMS ===

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String, // Subject (User/Cell)
    role: String,
    exp: usize,
}

// === SERVICE ===

struct IamState {
    users: HashMap<String, String>, // ClientID -> Secret (Hash)
    roles: HashMap<String, HashSet<String>>, // Role -> { "resource:action" }
    enc_key: EncodingKey,
    dec_key: DecodingKey,
}

#[service]
#[derive(Clone)]
struct IamService {
    state: Arc<RwLock<IamState>>,
}

impl IamService {
    fn new() -> Self {
        let secret = b"super_secret_enterprise_key_do_not_commit";
        
        let mut users = HashMap::new();
        users.insert("admin".into(), "admin123".into());
        users.insert("finance".into(), "moneyprinter".into());
        users.insert("observer".into(), "watcher".into());

        let mut roles = HashMap::new();
        
        let mut admin_perms = HashSet::new();
        admin_perms.insert("*".into());
        roles.insert("admin".into(), admin_perms);
        
        let mut finance_perms = HashSet::new();
        finance_perms.insert("ledger:deposit".into());
        finance_perms.insert("ledger:withdraw".into());
        finance_perms.insert("vault:read".into());
        roles.insert("finance".into(), finance_perms);

        Self {
            state: Arc::new(RwLock::new(IamState {
                users,
                roles,
                enc_key: EncodingKey::from_secret(secret),
                dec_key: DecodingKey::from_secret(secret),
            })),
        }
    }
}

#[handler]
impl IamService {
    async fn login(&self, req: LoginRequest) -> Result<AuthResponse> {
        let state = self.state.read().await;
        
        if let Some(expected_secret) = state.users.get(&req.client_id) {
            if expected_secret == &req.client_secret {
                // Determine Role (simplified mapping)
                let role = if req.client_id == "admin" { "admin" } else { &req.client_id };
                
                let expiration = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_secs() as usize + 3600; // 1 hour

                let claims = Claims {
                    sub: req.client_id.clone(),
                    role: role.to_string(),
                    exp: expiration,
                };

                let token = encode(&Header::default(), &claims, &state.enc_key)
                    .map_err(|_| anyhow::anyhow!("Token generation failed"))?;

                tracing::info!("[IAM] Issued token for '{}' (Role: {})", req.client_id, role);
                return Ok(AuthResponse {
                    token,
                    expires_in: 3600,
                });
            }
        }
        
        tracing::warn!("[IAM] Login failed for '{}'", req.client_id);
        bail!("Invalid credentials")
    }

    async fn check(&self, req: CheckPermission) -> Result<bool> {
        let state = self.state.read().await;
        
        // 1. Validate Token
        let token_data = decode::<Claims>(&req.token, &state.dec_key, &Validation::default())
            .map_err(|_| anyhow::anyhow!("Invalid token"))?;
            
        let role = token_data.claims.role;
        
        // 2. Check RBAC
        if let Some(perms) = state.roles.get(&role) {
            if perms.contains("*") {
                return Ok(true);
            }
            let required = format!("{}:{}", req.resource, req.action);
            if perms.contains(&required) {
                return Ok(true);
            }
        }
        
        tracing::info!("[IAM] Access Denied: {} -> {} (Role: {})", token_data.claims.sub, req.resource, role);
        Ok(false)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    tracing::info!("[IAM] Policy Engine Active");
    
    let service = IamService::new();
    service.serve("iam").await
}