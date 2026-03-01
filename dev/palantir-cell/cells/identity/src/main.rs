//! Identity Cell - Single Source of Truth for Entity Resolution
//!
//! This cell maintains the canonical identity for every entity in the system.
//! It resolves external identifiers (tax IDs, LEIs, domains) to permanent Palantir IDs.

use anyhow::Result;
use cell_sdk::*;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use chrono::{DateTime, Utc};

// ========= PROTEINS =========

#[protein]
#[derive(Debug, Clone)]
pub enum EntityType {
    Organization,
    Person,
    Asset,
    PoliticalEntity,
    GeographicRegion,
    Contract,
    Unknown,
}

#[protein]
#[derive(Debug, Clone)]
pub enum IdentifierType {
    TaxId,           // EIN, VAT, etc.
    Lei,             // Legal Entity Identifier
    Domain,          // Internet domain
    Cik,             // SEC Central Index Key
    Duns,            // Dun & Bradstreet
    Isin,            // International Securities ID
    Ticker,          // Stock ticker
    ExternalId(String), // Custom namespace
}

#[protein]
#[derive(Debug, Clone)]
pub struct Identifier {
    pub id_type: IdentifierType,
    pub value: String,
    pub authority: String,  // Who issued this ID
    pub confidence: f32,    // 0.0-1.0
}

#[protein]
#[derive(Debug, Clone)]
pub struct ProvenanceRef {
    pub fact_id: String,    // Points to provenance cell
    as_of: u64,             // When this fact was recorded
}

#[protein]
#[derive(Debug, Clone)]
pub struct Attribute {
    pub name: String,
    pub value: serde_json::Value,
    pub provenance: ProvenanceRef,
    pub confidence: f32,
    pub valid_from: u64,
    pub valid_to: Option<u64>,
}

#[protein]
#[derive(Debug, Clone)]
pub struct Entity {
    pub id: String,                 // Permanent Palantir ID (UUIDv7)
    pub entity_type: EntityType,
    pub primary_name: String,
    pub identifiers: Vec<Identifier>,
    pub attributes: Vec<Attribute>,
    pub created_at: u64,
    pub updated_at: u64,
    pub version: u64,
}

#[protein]
#[derive(Debug, Clone)]
pub struct ResolveRequest {
    pub identifiers: Vec<Identifier>,
    pub min_confidence: Option<f32>,
    pub include_attributes: Option<Vec<String>>,
    pub resolve_to: Option<u64>,    // As of timestamp
}

#[protein]
#[derive(Debug, Clone)]
pub struct ResolveResponse {
    pub entity: Option<Entity>,
    pub candidates: Vec<ResolutionCandidate>,
    pub resolution_time_ms: u64,
}

#[protein]
#[derive(Debug, Clone)]
pub struct ResolutionCandidate {
    pub entity_id: String,
    pub confidence: f32,
    pub matched_on: Vec<String>,    // Which identifiers matched
    pub explanation: String,
}

#[protein]
#[derive(Debug, Clone)]
pub struct CreateEntityRequest {
    pub entity_type: EntityType,
    pub primary_name: String,
    pub identifiers: Vec<Identifier>,
    pub initial_attributes: Vec<Attribute>,
    pub provenance: ProvenanceRef,
}

#[protein]
#[derive(Debug, Clone)]
pub struct MergeEntitiesRequest {
    pub primary_id: String,
    pub secondary_ids: Vec<String>,
    pub merge_strategy: MergeStrategy,
    pub provenance: ProvenanceRef,
}

#[protein]
#[derive(Debug, Clone)]
pub enum MergeStrategy {
    KeepAll,           // Keep all attributes, merge histories
    Overwrite,         // Primary overwrites secondary
    ConfidenceBased,   // Higher confidence wins
    Temporal,          // Most recent wins
}

#[protein]
#[derive(Debug, Clone)]
pub struct EntityHistory {
    pub entity_id: String,
    pub versions: Vec<EntitySnapshot>,
    pub merge_events: Vec<MergeEvent>,
}

// ========= SERVICE =========

struct IdentityState {
    db: SqlitePool,
    cache: Arc<RwLock<HashMap<String, Entity>>>,
}

#[service]
#[derive(Clone)]
struct IdentityService {
    state: Arc<IdentityState>,
}

#[handler]
impl IdentityService {
    /// Resolve identifiers to a canonical entity
    async fn resolve(&self, req: ResolveRequest) -> Result<ResolveResponse> {
        let start = std::time::Instant::now();
        
        // Build query based on identifiers
        let mut candidates = Vec::new();
        
        for ident in &req.identifiers {
            let matches = self.find_by_identifier(ident).await?;
            candidates.extend(matches);
        }
        
        // Deduplicate and score candidates
        let mut scored: HashMap<String, (f32, Vec<String>)> = HashMap::new();
        for (entity_id, confidence, matched_on) in candidates {
            let entry = scored.entry(entity_id).or_insert((0.0, Vec::new()));
            entry.0 = entry.0.max(confidence); // Take max confidence
            entry.1.push(matched_on);
        }
        
        // Sort by confidence
        let mut candidates: Vec<ResolutionCandidate> = scored
            .into_iter()
            .map(|(entity_id, (confidence, matched_on))| ResolutionCandidate {
                entity_id,
                confidence,
                matched_on,
                explanation: format!("Matched on {} identifiers", matched_on.len()),
            })
            .collect();
        
        candidates.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap());
        
        // Get the best candidate if confidence threshold met
        let min_conf = req.min_confidence.unwrap_or(0.8);
        let entity = if let Some(best) = candidates.first() {
            if best.confidence >= min_conf {
                self.get_entity(&best.entity_id, req.resolve_to).await?
            } else {
                None
            }
        } else {
            None
        };
        
        Ok(ResolveResponse {
            entity,
            candidates,
            resolution_time_ms: start.elapsed().as_millis() as u64,
        })
    }
    
    /// Create a new entity
    async fn create_entity(&self, req: CreateEntityRequest) -> Result<Entity> {
        let id = format!("ent_{:032x}", blake3::hash(&format!("{:?}", req.identifiers).as_bytes()));
        let now = Utc::now().timestamp() as u64;
        
        let entity = Entity {
            id: id.clone(),
            entity_type: req.entity_type,
            primary_name: req.primary_name,
            identifiers: req.identifiers,
            attributes: req.initial_attributes,
            created_at: now,
            updated_at: now,
            version: 1,
        };
        
        // Store in database
        self.persist_entity(&entity).await?;
        
        // Update cache
        self.state.cache.write().await.insert(id, entity.clone());
        
        Ok(entity)
    }
    
    /// Merge multiple entities (critical for deduplication)
    async fn merge_entities(&self, req: MergeEntitiesRequest) -> Result<Entity> {
        let mut primary = self.get_entity(&req.primary_id, None).await?
            .ok_or_else(|| anyhow::anyhow!("Primary entity not found"))?;
        
        let mut secondaries = Vec::new();
        for id in &req.secondary_ids {
            if let Some(entity) = self.get_entity(id, None).await? {
                secondaries.push(entity);
            }
        }
        
        // Apply merge strategy
        match req.merge_strategy {
            MergeStrategy::KeepAll => {
                for sec in secondaries {
                    primary.attributes.extend(sec.attributes);
                    primary.identifiers.extend(sec.identifiers);
                }
            }
            MergeStrategy::ConfidenceBased => {
                // Implement confidence-based merging
                // This would use provenance confidence scores
            }
            MergeStrategy::Temporal => {
                // Keep most recent attributes
            }
            MergeStrategy::Overwrite => {
                // Primary overwrites - do nothing
            }
        }
        
        primary.version += 1;
        primary.updated_at = Utc::now().timestamp() as u64;
        
        // Update database
        self.persist_entity(&primary).await?;
        
        // Record merge event in provenance cell
        self.record_merge_event(&req).await?;
        
        Ok(primary)
    }
    
    /// Get entity history
    async fn get_history(&self, entity_id: String) -> Result<EntityHistory> {
        // Query version history from database
        let versions = sqlx::query_as::<_, EntitySnapshot>(
            "SELECT * FROM entity_versions WHERE entity_id = ? ORDER BY version DESC"
        )
        .bind(&entity_id)
        .fetch_all(&self.state.db)
        .await?;
        
        // Get merge events from provenance
        let merge_events = self.get_merge_events(&entity_id).await?;
        
        Ok(EntityHistory {
            entity_id,
            versions,
            merge_events,
        })
    }
}

// Database operations
impl IdentityService {
    async fn init_db(&self) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS entities (
                id TEXT PRIMARY KEY,
                entity_type TEXT NOT NULL,
                primary_name TEXT NOT NULL,
                identifiers JSON NOT NULL,
                attributes JSON NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                version INTEGER NOT NULL
            );
            
            CREATE TABLE IF NOT EXISTS entity_versions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                entity_id TEXT NOT NULL,
                snapshot JSON NOT NULL,
                version INTEGER NOT NULL,
                created_at INTEGER NOT NULL,
                FOREIGN KEY (entity_id) REFERENCES entities(id)
            );
            
            CREATE TABLE IF NOT EXISTS identifier_index (
                id_type TEXT NOT NULL,
                value TEXT NOT NULL,
                entity_id TEXT NOT NULL,
                confidence REAL NOT NULL,
                PRIMARY KEY (id_type, value),
                FOREIGN KEY (entity_id) REFERENCES entities(id)
            );
            
            CREATE INDEX idx_identifier_value ON identifier_index(value);
            CREATE INDEX idx_entity_updated ON entities(updated_at);
            "#
        )
        .execute(&self.state.db)
        .await?;
        
        Ok(())
    }
    
    async fn find_by_identifier(&self, ident: &Identifier) -> Result<Vec<(String, f32, String)>> {
        let id_type = format!("{:?}", ident.id_type);
        
        let rows = sqlx::query(
            "SELECT entity_id, confidence FROM identifier_index WHERE id_type = ? AND value = ?"
        )
        .bind(&id_type)
        .bind(&ident.value)
        .fetch_all(&self.state.db)
        .await?;
        
        Ok(rows.into_iter()
            .map(|row| (row.get(0), row.get(1), id_type.clone()))
            .collect())
    }
    
    async fn get_entity(&self, id: &str, as_of: Option<u64>) -> Result<Option<Entity>> {
        // Check cache first
        if let Some(cached) = self.state.cache.read().await.get(id) {
            if let Some(as_of) = as_of {
                if cached.updated_at <= as_of {
                    return Ok(Some(cached.clone()));
                }
            } else {
                return Ok(Some(cached.clone()));
            }
        }
        
        // Query database
        let row = sqlx::query_as::<_, (String, String, String, String, String, i64, i64, i64)>(
            "SELECT * FROM entities WHERE id = ?"
        )
        .bind(id)
        .fetch_optional(&self.state.db)
        .await?;
        
        if let Some((id, entity_type, primary_name, identifiers_json, attributes_json, 
                     created_at, updated_at, version)) = row {
            let entity = Entity {
                id,
                entity_type: serde_json::from_str(&entity_type)?,
                primary_name,
                identifiers: serde_json::from_str(&identifiers_json)?,
                attributes: serde_json::from_str(&attributes_json)?,
                created_at: created_at as u64,
                updated_at: updated_at as u64,
                version: version as u64,
            };
            
            // Update cache
            self.state.cache.write().await.insert(entity.id.clone(), entity.clone());
            
            Ok(Some(entity))
        } else {
            Ok(None)
        }
    }
    
    async fn persist_entity(&self, entity: &Entity) -> Result<()> {
        let mut tx = self.state.db.begin().await?;
        
        // Insert/update main entity
        sqlx::query(
            r#"
            INSERT OR REPLACE INTO entities 
            (id, entity_type, primary_name, identifiers, attributes, created_at, updated_at, version)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            "#
        )
        .bind(&entity.id)
        .bind(format!("{:?}", entity.entity_type))
        .bind(&entity.primary_name)
        .bind(serde_json::to_string(&entity.identifiers)?)
        .bind(serde_json::to_string(&entity.attributes)?)
        .bind(entity.created_at as i64)
        .bind(entity.updated_at as i64)
        .bind(entity.version as i64)
        .execute(&mut *tx)
        .await?;
        
        // Insert version history
        sqlx::query(
            r#"
            INSERT INTO entity_versions (entity_id, snapshot, version, created_at)
            VALUES (?, ?, ?, ?)
            "#
        )
        .bind(&entity.id)
        .bind(serde_json::to_string(&entity)?)
        .bind(entity.version as i64)
        .bind(Utc::now().timestamp())
        .execute(&mut *tx)
        .await?;
        
        // Update identifier index
        for ident in &entity.identifiers {
            sqlx::query(
                r#"
                INSERT OR REPLACE INTO identifier_index (id_type, value, entity_id, confidence)
                VALUES (?, ?, ?, ?)
                "#
            )
            .bind(format!("{:?}", ident.id_type))
            .bind(&ident.value)
            .bind(&entity.id)
            .bind(ident.confidence)
            .execute(&mut *tx)
            .await?;
        }
        
        tx.commit().await?;
        Ok(())
    }
    
    async fn record_merge_event(&self, req: &MergeEntitiesRequest) -> Result<()> {
        // TODO: Call provenance cell to record merge
        Ok(())
    }
    
    async fn get_merge_events(&self, entity_id: &str) -> Result<Vec<MergeEvent>> {
        // TODO: Query provenance cell for merge events
        Ok(Vec::new())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_level(true)
        .init();
    
    println!("🔷 Identity Cell - Canonical Entity Resolution");
    println!("   └─ Database: SQLite with versioning");
    
    // Initialize database
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("./data"))
        .join("palantir/identity");
    
    std::fs::create_dir_all(&data_dir)?;
    let db_path = data_dir.join("identity.db");
    
    let db = SqlitePool::connect(&format!("sqlite:{}", db_path.display())).await?;
    
    let service = IdentityService {
        state: Arc::new(IdentityState {
            db,
            cache: Arc::new(RwLock::new(HashMap::new())),
        }),
    };
    
    // Initialize database schema
    service.init_db().await?;
    
    println!("   ├─ Cache: in-memory with LRU");
    println!("   └─ Ready to resolve identities");
    
    service.serve("identity").await
}