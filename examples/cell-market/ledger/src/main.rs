// Path: /Users/07lead01/cell/examples/cell-market/ledger/src/main.rs
use anyhow::Result;
use cell_sdk::{service, handler, protein, Runtime};
use cell_model::macro_coordination::{MacroInfo, MacroKind};
use dashmap::DashMap;
use std::sync::Arc;
use tracing::info;

// --- DNA ---
#[protein]
pub enum Asset { USD, BTC }

// --- LOGIC ---
struct LedgerState {
    accounts: DashMap<u64, DashMap<String, u64>>,
}

#[service]
#[derive(Clone)]
struct LedgerService {
    state: Arc<LedgerState>,
}

#[handler]
impl LedgerService {
    async fn deposit(&self, user: u64, asset: Asset, amount: u64) -> Result<u64> {
        let key = format!("{:?}", asset);
        let user_map = self.state.accounts.entry(user).or_insert_with(DashMap::new);
        let mut bal = user_map.entry(key).or_insert(0);
        *bal += amount;
        info!("[Ledger] Deposit: User {} +{} {:?}", user, amount, asset);
        Ok(*bal)
    }

    async fn lock_funds(&self, user: u64, asset: Asset, amount: u64) -> Result<bool> {
        let key = format!("{:?}", asset);
        if let Some(user_map) = self.state.accounts.get_mut(&user) {
            if let Some(mut bal) = user_map.get_mut(&key) {
                if *bal >= amount {
                    *bal -= amount;
                    info!("[Ledger] Locked: User {} -{} {:?}", user, amount, asset);
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    info!("[Ledger] Online - Macro Provider Active");
    
    let service = LedgerService { state: Arc::new(LedgerState { accounts: DashMap::new() }) };

    // Define macros provided by this Cell
    let macros = vec![
        MacroInfo {
            name: "table".to_string(),
            kind: MacroKind::Attribute,
            description: "Generates a thread-safe in-memory database table with CRUD operations".to_string(),
            dependencies: vec![], // In a real scenario, we might list dependencies the client needs
        },
    ];

    // --- THE COMPILER PLUGIN LOGIC ---
    let expander = |name: &str, context: &cell_model::macro_coordination::ExpansionContext| -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send>> {
        let name_owned = name.to_string();
        let ctx = context.clone();
        
        Box::pin(async move {
            if name_owned == "table" {
                let struct_name = &ctx.struct_name;
                let table_name = format!("{}Table", struct_name);
                
                // Heuristic: Assume the first field is the Primary Key
                let (pk_name, pk_type) = ctx.fields.first()
                    .ok_or_else(|| anyhow::anyhow!("Struct must have at least one field to be a table"))?;

                // Reconstruct fields definition
                let fields_def = ctx.fields.iter()
                    .map(|(n, t)| format!("pub {}: {},", n, t))
                    .collect::<Vec<_>>()
                    .join("\n    ");

                // Generate the robust database code
                let code = format!(r#"
                    // 1. The Data Structure (Enhanced with Cell traits)
                    #[derive(Clone, Debug, PartialEq, 
                        cell_sdk::serde::Serialize, cell_sdk::serde::Deserialize,
                        cell_sdk::rkyv::Archive, cell_sdk::rkyv::Serialize, cell_sdk::rkyv::Deserialize
                    )]
                    #[archive(check_bytes)]
                    #[archive(crate = "cell_sdk::rkyv")]
                    #[serde(crate = "cell_sdk::serde")]
                    pub struct {struct_name} {{
                        {fields_def}
                    }}

                    // 2. The Table Manager (DAO)
                    // We use Arc<RwLock<HashMap>> to ensure it works with standard std lib
                    // without forcing the consumer to add extra crates like dashmap.
                    #[derive(Clone)]
                    pub struct {table_name} {{
                        storage: std::sync::Arc<std::sync::RwLock<std::collections::HashMap<{pk_type}, {struct_name}>>>,
                    }}

                    impl {table_name} {{
                        pub fn new() -> Self {{
                            Self {{
                                storage: std::sync::Arc::new(std::sync::RwLock::new(std::collections::HashMap::new())),
                            }}
                        }}

                        /// Create or Update (Upsert)
                        pub fn save(&self, item: {struct_name}) {{
                            let mut guard = self.storage.write().unwrap();
                            // PK is {pk_name}
                            guard.insert(item.{pk_name}.clone(), item);
                        }}

                        /// Read
                        pub fn get(&self, id: &{pk_type}) -> Option<{struct_name}> {{
                            let guard = self.storage.read().unwrap();
                            guard.get(id).cloned()
                        }}

                        /// Delete
                        pub fn remove(&self, id: &{pk_type}) -> Option<{struct_name}> {{
                            let mut guard = self.storage.write().unwrap();
                            guard.remove(id)
                        }}

                        /// List All
                        pub fn all(&self) -> Vec<{struct_name}> {{
                            let guard = self.storage.read().unwrap();
                            guard.values().cloned().collect()
                        }}

                        /// Count
                        pub fn count(&self) -> usize {{
                            let guard = self.storage.read().unwrap();
                            guard.len()
                        }}
                    }}
                "#, 
                struct_name = struct_name,
                table_name = table_name,
                fields_def = fields_def,
                pk_name = pk_name,
                pk_type = pk_type
                );

                Ok(code)
            } else {
                Err(anyhow::anyhow!("Unknown macro"))
            }
        })
    };

    Runtime::ignite_with_coordination::<_, LedgerServiceProtocol, LedgerServiceResponse, _>(
        move |req| {
            let svc = service.clone();
            Box::pin(async move { svc.dispatch(req).await })
        },
        "ledger",
        macros,
        expander
    ).await
}