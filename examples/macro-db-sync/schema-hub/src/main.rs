// examples/macro-db-sync/schema-hub/src/main.rs
use anyhow::{Result};
use cell_sdk::prelude::*; // Fixed: Import Runtime via prelude
use cell_sdk::{service, handler};
use cell_model::macro_coordination::{MacroInfo, MacroKind, ExpansionContext};
use std::fs;
use std::path::PathBuf;

// Service stub
#[service]
#[derive(Clone)]
struct SchemaService;

#[handler]
impl SchemaService {
    // Fixed: Added a dummy method to prevent "zero-variant enum" errors 
    // in the generated protocol serialization code.
    async fn ping(&self) -> Result<String> {
        Ok("pong".to_string())
    }
}

fn get_storage_paths() -> (PathBuf, PathBuf) {
    let home = dirs::home_dir().expect("HOME not found");
    let schema_dir = home.join(".cell/demo/schemas");
    let data_dir = home.join(".cell/demo/data");
    fs::create_dir_all(&schema_dir).ok();
    fs::create_dir_all(&data_dir).ok();
    (schema_dir, data_dir)
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging so we can see output
    tracing_subscriber::fmt().with_target(false).init();

    let service = SchemaService;

    // Define the macro capability
    let macros = vec![
        MacroInfo {
            name: "shared_table".to_string(),
            kind: MacroKind::Attribute,
            description: "Syncs schema and generates file-backed DAO".to_string(),
            dependencies: vec![],
        },
    ];

    // The Logic run at Compile Time by Consumers
    let expander = |name: &str, ctx: &ExpansionContext| -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send>> {
        let name = name.to_string();
        let mut ctx = ctx.clone();

        Box::pin(async move {
            if name != "shared_table" {
                return Err(anyhow::anyhow!("Unknown macro"));
            }

            let (schema_dir, _data_dir) = get_storage_paths();
            let schema_file = schema_dir.join(format!("{}.json", ctx.struct_name));

            // --- 1. Schema Synchronization ---
            if !ctx.fields.is_empty() {
                // CASE A: Declaration (Source of Truth)
                // Save schema to disk so consumers can find it
                let json = serde_json::to_string_pretty(&ctx.fields)?;
                fs::write(&schema_file, json)?;
            } else {
                // CASE B: Consumption (Phantom Struct)
                // Load schema from disk
                if !schema_file.exists() {
                    return Err(anyhow::anyhow!(
                        "Schema for '{}' not defined yet. Run the producer crate first!", 
                        ctx.struct_name
                    ));
                }
                let json = fs::read_to_string(&schema_file)?;
                let fields: Vec<(String, String)> = serde_json::from_str(&json)?;
                ctx.fields = fields;
            }

            // --- 2. Code Generation ---
            let struct_name = &ctx.struct_name;
            let table_name = format!("{}Table", struct_name);
            let (pk_name, pk_type) = ctx.fields.first()
                .ok_or_else(|| anyhow::anyhow!("Struct must have at least one field"))?;

            // Generate struct fields
            let fields_def = ctx.fields.iter()
                .map(|(n, t)| format!("pub {}: {},", n, t))
                .collect::<Vec<_>>()
                .join("\n    ");

            // Generate the code
            // We inject a simple JSON file-based database implementation
            Ok(format!(r#"
                #[derive(Clone, Debug, PartialEq, cell_sdk::serde::Serialize, cell_sdk::serde::Deserialize)]
                pub struct {struct_name} {{
                    {fields_def}
                }}

                pub struct {table_name} {{
                    path: std::path::PathBuf,
                }}

                impl {table_name} {{
                    pub fn new() -> Self {{
                        let home = dirs::home_dir().unwrap();
                        let path = home.join(".cell/demo/data/{struct_name}.json");
                        if !path.exists() {{
                            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                            std::fs::write(&path, "[]").unwrap();
                        }}
                        Self {{ path }}
                    }}

                    fn load(&self) -> Vec<{struct_name}> {{
                        let data = std::fs::read_to_string(&self.path).unwrap_or_else(|_| "[]".into());
                        cell_sdk::serde_json::from_str(&data).unwrap_or_default()
                    }}

                    fn save_all(&self, items: &[{struct_name}]) {{
                        let json = cell_sdk::serde_json::to_string_pretty(items).unwrap();
                        std::fs::write(&self.path, json).unwrap();
                    }}

                    pub fn save(&self, item: {struct_name}) {{
                        let mut items = self.load();
                        // simplistic upsert based on PK
                        if let Some(idx) = items.iter().position(|i| i.{pk_name} == item.{pk_name}) {{
                            items[idx] = item;
                        }} else {{
                            items.push(item);
                        }}
                        self.save_all(&items);
                    }}

                    pub fn get(&self, id: &{pk_type}) -> Option<{struct_name}> {{
                        let items = self.load();
                        items.into_iter().find(|i| &i.{pk_name} == id)
                    }}

                    pub fn all(&self) -> Vec<{struct_name}> {{
                        self.load()
                    }}
                }}
            "#, 
            struct_name = struct_name,
            table_name = table_name,
            fields_def = fields_def,
            pk_name = pk_name,
            pk_type = pk_type
            ))
        })
    };

    Runtime::ignite_with_coordination::<_, SchemaServiceProtocol, SchemaServiceResponse, _>(
        move |req| {
            let svc = service.clone();
            Box::pin(async move { svc.dispatch(req).await })
        },
        "schema-hub",
        macros,
        expander
    ).await
}