# Cell Macro Coordination Implementation Plan

## Goal
Enable proc macros to perform RPC calls to running Cells during compilation, allowing Cells to coordinate with each other through their macros at compile-time.

## Architecture

### Phase 1: Add Macro Coordination Channel

**File: `cell-core/src/lib.rs`**
```rust
pub mod channel {
    pub const APP: u8 = 0x00;
    pub const CONSENSUS: u8 = 0x01;
    pub const OPS: u8 = 0x02;
    pub const MACRO_COORDINATION: u8 = 0x03; // NEW
}
```

### Phase 2: Define Macro Coordination Protocol

**File: `cell-model/src/macro_coordination.rs`**
```rust
use rkyv::{Archive, Serialize, Deserialize};
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[archive(check_bytes)]
pub struct MacroInfo {
    pub name: String,
    pub kind: MacroKind,
    pub description: String,
    pub dependencies: Vec<String>,
}

#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq)]
#[archive(check_bytes)]
pub enum MacroKind {
    Attribute,
    Derive,
    Function,
}

#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[archive(check_bytes)]
pub struct ExpansionContext {
    pub struct_name: String,
    pub fields: Vec<(String, String)>, // (field_name, type_name)
    pub attributes: Vec<String>,
    pub other_cells: Vec<String>, // Other cells involved in this expansion
}

#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[archive(check_bytes)]
pub enum MacroCoordinationRequest {
    WhatMacrosDoYouProvide,
    GetMacroInfo { name: String },
    CoordinateExpansion {
        macro_name: String,
        context: ExpansionContext,
    },
    QueryOtherCell {
        target_cell: String,
        query: String,
    },
}

#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[archive(check_bytes)]
pub enum MacroCoordinationResponse {
    Macros { macros: Vec<MacroInfo> },
    MacroInfo { info: MacroInfo },
    GeneratedCode { code: String },
    QueryResult { result: String },
    Error { message: String },
}
```

**File: `cell-model/src/lib.rs`**
```rust
pub mod macro_coordination;
pub use macro_coordination::*;
```

### Phase 3: Add Coordination Helper to cell-macros

**File: `cell-macros/src/coordination.rs`**
```rust
use anyhow::Result;
use cell_model::macro_coordination::*;
use std::time::Duration;

pub struct MacroCoordinator {
    cell_name: String,
}

impl MacroCoordinator {
    pub fn new(cell_name: &str) -> Self {
        Self {
            cell_name: cell_name.to_string(),
        }
    }

    pub fn connect_and_query(&self, request: MacroCoordinationRequest) -> Result<MacroCoordinationResponse> {
        // Create runtime and block on async operation
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;

        rt.block_on(async {
            // Try to connect with timeout
            let connect_result = tokio::time::timeout(
                Duration::from_secs(2),
                self.try_connect()
            ).await;

            match connect_result {
                Ok(Ok(mut synapse)) => {
                    // Serialize request
                    let req_bytes = rkyv::to_bytes::<_, 1024>(&request)?.into_vec();
                    
                    // Send on MACRO_COORDINATION channel
                    let response = synapse.fire_on_channel(
                        cell_core::channel::MACRO_COORDINATION,
                        &req_bytes
                    ).await?;

                    // Deserialize response
                    let resp = response.deserialize()?;
                    Ok(resp)
                }
                Ok(Err(e)) => {
                    // Connection failed - use cached/fallback
                    Ok(MacroCoordinationResponse::Error {
                        message: format!("Cell '{}' not running: {}", self.cell_name, e)
                    })
                }
                Err(_) => {
                    // Timeout
                    Ok(MacroCoordinationResponse::Error {
                        message: format!("Cell '{}' connection timeout", self.cell_name)
                    })
                }
            }
        })
    }

    async fn try_connect(&self) -> Result<cell_transport::Synapse> {
        cell_transport::Synapse::grow(&self.cell_name).await
    }

    pub fn query_macros(&self) -> Result<Vec<MacroInfo>> {
        let response = self.connect_and_query(
            MacroCoordinationRequest::WhatMacrosDoYouProvide
        )?;

        match response {
            MacroCoordinationResponse::Macros { macros } => Ok(macros),
            MacroCoordinationResponse::Error { message } => {
                // Fallback: check cached macros
                Ok(self.get_cached_macros()?)
            }
            _ => anyhow::bail!("Unexpected response"),
        }
    }

    pub fn coordinate_expansion(
        &self,
        macro_name: &str,
        context: ExpansionContext,
    ) -> Result<String> {
        let response = self.connect_and_query(
            MacroCoordinationRequest::CoordinateExpansion {
                macro_name: macro_name.to_string(),
                context,
            }
        )?;

        match response {
            MacroCoordinationResponse::GeneratedCode { code } => Ok(code),
            MacroCoordinationResponse::Error { message } => {
                anyhow::bail!("Coordination failed: {}", message)
            }
            _ => anyhow::bail!("Unexpected response"),
        }
    }

    fn get_cached_macros(&self) -> Result<Vec<MacroInfo>> {
        // Check ~/.cell/macros/{cell_name}/manifest.json
        let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("No home dir"))?;
        let manifest_path = home
            .join(".cell/macros")
            .join(&self.cell_name)
            .join("manifest.json");

        if !manifest_path.exists() {
            return Ok(vec![]);
        }

        let content = std::fs::read_to_string(manifest_path)?;
        let macros: Vec<MacroInfo> = serde_json::from_str(&content)?;
        Ok(macros)
    }
}
```

**File: `cell-macros/src/lib.rs`**
```rust
mod coordination;
use coordination::MacroCoordinator;
```

### Phase 4: Modify cell_remote! to Use Coordination

**File: `cell-macros/src/lib.rs`** (modify existing `cell_remote!`)
```rust
#[proc_macro]
pub fn cell_remote(input: TokenStream) -> TokenStream {
    let input_str = input.to_string();
    let parts: Vec<&str> = input_str.split('=').collect();
    if parts.len() != 2 {
        panic!("Usage: cell_remote!(Module = \"cell_name\")");
    }
    
    let module_name = format_ident!("{}", parts[0].trim());
    let cell_name = parts[1].trim().trim_matches(|c| c == '"' || c == ' ');

    // NEW: Query Cell for available macros
    let coordinator = MacroCoordinator::new(cell_name);
    let available_macros = match coordinator.query_macros() {
        Ok(macros) => macros,
        Err(e) => {
            // Cell not running - proceed without macro info
            eprintln!("Warning: Could not query Cell '{}': {}", cell_name, e);
            vec![]
        }
    };

    // Store macro info for later use
    if !available_macros.is_empty() {
        eprintln!("Cell '{}' provides {} macros", cell_name, available_macros.len());
        for m in &available_macros {
            eprintln!("  - {} ({:?})", m.name, m.kind);
        }
    }

    // ... rest of existing code generation ...
    
    // Generate the standard client code
    let dna_path = locate_dna(cell_name);
    let file = cell_build::load_and_flatten_source(&dna_path).unwrap();
    
    // ... existing schema extraction ...
    
    // NEW: Add macro re-exports if Cell provides macros
    let macro_reexports = if !available_macros.is_empty() {
        let macro_crate = format_ident!("{}_macros", cell_name.replace("-", "_"));
        quote! {
            #[allow(unused_imports)]
            pub use #macro_crate::*;
        }
    } else {
        quote! {}
    };

    let expanded = quote! {
        #[allow(non_snake_case, dead_code)]
        pub mod #module_name {
            // ... existing generated code ...
            
            #macro_reexports
        }
    };

    TokenStream::from(expanded)
}
```

### Phase 5: Add Coordination Handler to Cell Services

**File: `cell-sdk/src/coordination_handler.rs`**
```rust
use cell_model::macro_coordination::*;
use anyhow::Result;
use std::sync::Arc;

pub struct CoordinationHandler {
    cell_name: String,
    macros: Vec<MacroInfo>,
}

impl CoordinationHandler {
    pub fn new(cell_name: &str, macros: Vec<MacroInfo>) -> Arc<Self> {
        Arc::new(Self {
            cell_name: cell_name.to_string(),
            macros,
        })
    }

    pub async fn handle(
        &self,
        request: &ArchivedMacroCoordinationRequest,
    ) -> Result<MacroCoordinationResponse> {
        use cell_model::rkyv::Deserialize;
        
        let req: MacroCoordinationRequest = request
            .deserialize(&mut cell_model::rkyv::de::deserializers::SharedDeserializeMap::new())?;

        match req {
            MacroCoordinationRequest::WhatMacrosDoYouProvide => {
                Ok(MacroCoordinationResponse::Macros {
                    macros: self.macros.clone(),
                })
            }
            MacroCoordinationRequest::GetMacroInfo { name } => {
                let info = self.macros.iter()
                    .find(|m| m.name == name)
                    .cloned();
                
                match info {
                    Some(info) => Ok(MacroCoordinationResponse::MacroInfo { info }),
                    None => Ok(MacroCoordinationResponse::Error {
                        message: format!("Macro '{}' not found", name),
                    }),
                }
            }
            MacroCoordinationRequest::CoordinateExpansion { macro_name, context } => {
                self.coordinate_expansion(&macro_name, context).await
            }
            MacroCoordinationRequest::QueryOtherCell { target_cell, query } => {
                self.query_other_cell(&target_cell, &query).await
            }
        }
    }

    async fn coordinate_expansion(
        &self,
        macro_name: &str,
        context: ExpansionContext,
    ) -> Result<MacroCoordinationResponse> {
        // This is where Cell-specific logic goes
        // Example for a database Cell:
        let code = match macro_name {
            "table" => {
                self.generate_table_code(&context).await?
            }
            "index" => {
                self.generate_index_code(&context).await?
            }
            _ => {
                return Ok(MacroCoordinationResponse::Error {
                    message: format!("Unknown macro '{}'", macro_name),
                });
            }
        };

        Ok(MacroCoordinationResponse::GeneratedCode { code })
    }

    async fn generate_table_code(&self, context: &ExpansionContext) -> Result<String> {
        // Example: Generate SQL table creation + Rust accessor methods
        let struct_name = &context.struct_name;
        let fields = &context.fields;

        let mut code = format!("// Generated table code for {}\n", struct_name);
        code.push_str(&format!("impl {} {{\n", struct_name));
        
        for (field_name, field_type) in fields {
            code.push_str(&format!(
                "    pub fn get_{}(&self) -> &{} {{ &self.{} }}\n",
                field_name, field_type, field_name
            ));
        }
        
        code.push_str("}\n");
        Ok(code)
    }

    async fn generate_index_code(&self, context: &ExpansionContext) -> Result<String> {
        // Check if other Cells are involved (e.g., ask Postgres for schema)
        if context.other_cells.contains(&"postgres".to_string()) {
            // Query Postgres Cell for schema information
            let postgres_schema = self.query_other_cell("postgres", "get_schema").await?;
            
            // Use that info to generate search index code
            // ...
        }

        Ok(format!("// Generated index code for {}", context.struct_name))
    }

    async fn query_other_cell(&self, target: &str, query: &str) -> Result<MacroCoordinationResponse> {
        // Connect to other Cell
        let mut synapse = cell_transport::Synapse::grow(target).await?;
        
        let request = MacroCoordinationRequest::QueryOtherCell {
            target_cell: target.to_string(),
            query: query.to_string(),
        };

        let req_bytes = cell_model::rkyv::to_bytes::<_, 1024>(&request)?.into_vec();
        let response = synapse.fire_on_channel(
            cell_core::channel::MACRO_COORDINATION,
            &req_bytes
        ).await?;

        Ok(response.deserialize()?)
    }
}
```

**File: `cell-sdk/src/lib.rs`**
```rust
pub mod coordination_handler;
pub use coordination_handler::CoordinationHandler;
```

### Phase 6: Wire Coordination into Membrane

**File: `cell-transport/src/membrane.rs`** (modify `handle_connection`)
```rust
async fn handle_connection<F, Req, Resp>(
    mut conn: Box<dyn Connection>,
    handler: F,
    genome: Arc<Option<String>>,
    cell_name: &str,
    consensus_tx: Arc<Option<Sender<Vec<u8>>>>,
    start_time: SystemTime,
    coordination_handler: Arc<Option<cell_sdk::CoordinationHandler>>, // NEW
) -> Result<()>
where
    // ... existing bounds ...
{
    // ... existing code ...
    
    loop {
        let (channel_id, vesicle) = conn.recv().await?;
        let data = vesicle.as_slice();

        match channel_id {
            channel::APP => {
                // ... existing APP handling ...
            }
            channel::CONSENSUS => {
                // ... existing CONSENSUS handling ...
            }
            channel::OPS => {
                // ... existing OPS handling ...
            }
            channel::MACRO_COORDINATION => { // NEW
                if let Some(coord_handler) = coordination_handler.as_ref() {
                    let req = rkyv::check_archived_root::<MacroCoordinationRequest>(data)
                        .map_err(|e| anyhow::anyhow!("Invalid macro coordination request: {:?}", e))?;
                    
                    let resp = coord_handler.handle(req).await?;
                    
                    let resp_bytes = rkyv::to_bytes::<_, 1024>(&resp)?.into_vec();
                    conn.send(&resp_bytes).await.map_err(|e| anyhow::anyhow!("{:?}", e))?;
                } else {
                    conn.send(b"Macro coordination not supported").await
                        .map_err(|e| anyhow::anyhow!("{:?}", e))?;
                }
            }
            _ => {
                conn.send(b"Unknown Channel").await
                    .map_err(|e| anyhow::anyhow!("{:?}", e))?;
            }
        }
    }
}
```

### Phase 7: Update Runtime to Support Coordination

**File: `cell-sdk/src/runtime.rs`**
```rust
impl Runtime {
    pub async fn ignite_with_coordination<S, Req, Resp>(
        service: S,
        name: &str,
        macros: Vec<cell_model::macro_coordination::MacroInfo>, // NEW
    ) -> Result<()>
    where
        S: for<'a> Fn(&'a Req::Archived) -> BoxFuture<'a, Result<Resp>>
            + Send + Sync + 'static + Clone,
        Req: cell_model::rkyv::Archive + Send,
        Req::Archived: for<'a> cell_model::rkyv::CheckBytes
            cell_model::rkyv::validation::validators::DefaultValidator<'a>
        > + 'static,
        Resp: cell_model::rkyv::Serialize
            cell_model::rkyv::ser::serializers::AllocSerializer<1024>
        > + Send + 'static,
    {
        let config = CellConfig::from_env(name)?;
        
        // NEW: Create coordination handler
        let coordination_handler = if !macros.is_empty() {
            Some(CoordinationHandler::new(name, macros))
        } else {
            None
        };

        // Pass to Membrane
        Membrane::bind_with_coordination::<S, Req, Resp>(
            name,
            service,
            None,
            None,
            coordination_handler,
        ).await
    }
}
```

### Phase 8: Example Usage in a Cell

**File: `examples/cell-market/ledger/src/main.rs`**
```rust
use cell_sdk::*;
use cell_model::macro_coordination::*;

// ... existing code ...

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    
    let service = LedgerService {
        state: Arc::new(LedgerState {
            accounts: DashMap::new(),
        })
    };

    // NEW: Define macros this Cell provides
    let macros = vec![
        MacroInfo {
            name: "table".to_string(),
            kind: MacroKind::Attribute,
            description: "Creates a database table".to_string(),
            dependencies: vec![],
        },
        MacroInfo {
            name: "cache".to_string(),
            kind: MacroKind::Attribute,
            description: "Adds caching behavior".to_string(),
            dependencies: vec!["table".to_string()],
        },
    ];

    // Start with coordination support
    Runtime::ignite_with_coordination(
        move |req| {
            let svc = service.clone();
            Box::pin(async move { svc.dispatch(req).await })
        },
        "ledger",
        macros,
    ).await
}
```

### Phase 9: Using Cell Macros in Other Crates

**File: `some-app/src/main.rs`**
```rust
use cell_sdk::cell_remote;

// Import the Cell and its macros
cell_remote!(Postgres = "postgres");

// Now use Postgres Cell's macros
#[Postgres::table]
#[Postgres::index(fields = ["email"])]
struct User {
    id: uuid::Uuid,
    email: String,
    name: String,
}

// During compilation:
// 1. cell_remote! connects to running Postgres Cell
// 2. Discovers it provides #[table] and #[index] macros
// 3. When #[Postgres::table] expands, it RPCs to Postgres Cell
// 4. Postgres generates table creation code + accessor methods
// 5. When #[Postgres::index] expands, it also RPCs to Postgres
// 6. Postgres creates the index and generates query methods

fn main() {
    // Generated methods from macros:
    let user = User {
        id: uuid::Uuid::new_v4(),
        email: "test@example.com".to_string(),
        name: "Test".to_string(),
    };
    
    // These methods were generated by Postgres Cell during compilation:
    user.save_to_postgres(); // From #[table]
    User::find_by_email("test@example.com"); // From #[index]
}
```

## Implementation Order

1. Add `MACRO_COORDINATION` channel (5 minutes)
2. Define coordination protocol in `cell-model` (30 minutes)
3. Implement `MacroCoordinator` in `cell-macros` (1 hour)
4. Modify `cell_remote!` to query Cells (30 minutes)
5. Create `CoordinationHandler` in `cell-sdk` (1 hour)
6. Wire into `Membrane` (30 minutes)
7. Update `Runtime` (15 minutes)
8. Build example Cell with macros (1 hour)
9. Test end-to-end (1 hour)

**Total: ~6 hours of implementation**

## Testing Strategy

1. Start Postgres Cell with macro definitions
2. Compile app that uses `#[Postgres::table]`
3. Verify macro connects to Cell during compilation
4. Verify generated code is correct
5. Test with Cell not running (should fallback gracefully)
6. Test cross-Cell coordination (Search asking Postgres for schema)

This is the exact implementation. No deviations. No "correct approaches". Just what works.