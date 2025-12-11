// cells/config/src/main.rs
// SPDX-License-Identifier: MIT
// Centralized Configuration Management

use cell_sdk::*;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[protein]
pub struct ConfigItem {
    pub key: String,
    pub value: String,
    pub env: String,
    pub version: u64,
}

struct ConfigState {
    // Env -> Key -> Value
    store: HashMap<String, HashMap<String, ConfigItem>>,
}

#[service]
#[derive(Clone)]
struct ConfigService {
    state: Arc<RwLock<ConfigState>>,
}

#[handler]
impl ConfigService {
    async fn set(&self, env: String, key: String, value: String) -> Result<u64> {
        let mut state = self.state.write().await;
        let env_store = state.store.entry(env.clone()).or_insert_with(HashMap::new);
        
        let version = env_store.get(&key).map(|i| i.version + 1).unwrap_or(1);
        
        let item = ConfigItem {
            key: key.clone(),
            value,
            env,
            version,
        };
        
        env_store.insert(key, item);
        Ok(version)
    }

    async fn get(&self, env: String, key: String) -> Result<Option<ConfigItem>> {
        let state = self.state.read().await;
        if let Some(env_store) = state.store.get(&env) {
            Ok(env_store.get(&key).map(|i| ConfigItem {
                key: i.key.clone(),
                value: i.value.clone(),
                env: i.env.clone(),
                version: i.version,
            }))
        } else {
            Ok(None)
        }
    }
    
    async fn list(&self, env: String) -> Result<Vec<ConfigItem>> {
        let state = self.state.read().await;
        if let Some(env_store) = state.store.get(&env) {
            Ok(env_store.values().map(|i| ConfigItem {
                key: i.key.clone(),
                value: i.value.clone(),
                env: i.env.clone(),
                version: i.version,
            }).collect())
        } else {
            Ok(vec![])
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    tracing::info!("[Config] Configuration Service Active");
    
    let state = ConfigState { store: HashMap::new() };
    let service = ConfigService { state: Arc::new(RwLock::new(state)) };
    
    service.serve("config").await
}