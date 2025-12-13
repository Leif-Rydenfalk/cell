// examples/cell-schema-sync/database/src/main.rs
// SPDX-License-Identifier: MIT
// The Canonical Schema Registry and Code Synthesizer

use anyhow::Result;
use cell_sdk::{service, handler, Runtime};
use cell_model::macro_coordination::{MacroInfo, MacroKind, ExpansionContext};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tracing::info;

// Mock Service (does nothing at runtime for this example, the magic is in the macro expander)
struct DatabaseService;

#[service]
#[derive(Clone)]
struct DatabaseServiceStruct {
    _inner: Arc<DatabaseService>,
}

#[handler]
impl DatabaseServiceStruct {
    async fn ping(&self) -> Result<String> {
        Ok("pong".to_string())
    }
}

// In-memory schema storage
// In a real database cell, this would persist to disk/raft.
static SCHEMA_STORE: std::sync::OnceLock<RwLock<HashMap<String, ExpansionContext>>> = std::sync::OnceLock::new();

fn get_store() -> &'static RwLock<HashMap<String, ExpansionContext>> {
    SCHEMA_STORE.get_or_init(|| RwLock::new(HashMap::new()))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    info!("[Database] Schema Registry Online");

    let service = DatabaseServiceStruct { _inner: Arc::new(DatabaseService) };

    let macros = vec![
        MacroInfo {
            name: "table".to_string(),
            kind: MacroKind::Attribute,
            description: "Generates synchronized struct and DAO".to_string(),
            dependencies: vec![],
        },
    ];

    let expander = |name: &str, context: &ExpansionContext| -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send>> {
        let name_owned = name.to_string();
        let mut ctx = context.clone();
        
        Box::pin(async move {
            if name_owned == "table" {
                let store = get_store();
                
                // --- SCHEMA SYNCHRONIZATION LOGIC ---
                // If fields are provided, this is a DECLARATION. Update store.
                // If fields are empty, this is a CONSUMPTION. Retrieve from store.
                
                if !ctx.fields.is_empty() {
                    info!("[Database] Registering schema for '{}'", ctx.struct_name);
                    let mut guard = store.write().unwrap();
                    guard.insert(ctx.struct_name.clone(), ctx.clone());
                } else {
                    info!("[Database] Retrieving schema for '{}'", ctx.struct_name);
                    let guard = store.read().unwrap();
                    if let Some(existing) = guard.get(&ctx.struct_name) {
                        ctx = existing.clone(); // Hydrate the context from source of truth
                    } else {
                        return Err(anyhow::anyhow!("Schema '{}' not found in registry", ctx.struct_name));
                    }
                }

                // Generate Code
                let struct_name = &ctx.struct_name;
                let table_name = format!("{}Table", struct_name);
                
                let (pk_name, pk_type) = ctx.fields.first()
                    .ok_or_else(|| anyhow::anyhow!("Schema for {} has no fields", struct_name))?;

                let fields_def = ctx.fields.iter()
                    .map(|(n, t)| format!("pub {}: {},", n, t))
                    .collect::<Vec<_>>()
                    .join("\n    ");

                let code = format!(r#"
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

                        pub fn save(&self, item: {struct_name}) {{
                            let mut guard = self.storage.write().unwrap();
                            guard.insert(item.{pk_name}.clone(), item);
                        }}

                        pub fn get(&self, id: &{pk_type}) -> Option<{struct_name}> {{
                            let guard = self.storage.read().unwrap();
                            guard.get(id).cloned()
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

    Runtime::ignite_with_coordination::<_, DatabaseServiceStructProtocol, DatabaseServiceStructResponse, _>(
        move |req| {
            let svc = service.clone();
            Box::pin(async move { svc.dispatch(req).await })
        },
        "database",
        macros,
        expander
    ).await
}