// cells/state-manager/src/main.rs
// Persistent state storage for the mesh using SQLite + Write-Ahead Log

use cell_sdk::*;
use rusqlite::{Connection, params};
use std::sync::{Arc, Mutex};
use std::path::PathBuf;

#[protein]
pub struct StoreRequest {
    pub key: String,
    pub value: Vec<u8>,
    pub ttl_secs: Option<u64>,
}

#[protein]
pub struct FetchRequest {
    pub key: String,
}

#[protein]
pub struct StateEntry {
    pub key: String,
    pub value: Vec<u8>,
    pub version: u64,
    pub timestamp: u64,
}

struct StateDb {
    conn: Arc<Mutex<Connection>>,
}

impl StateDb {
    fn new(path: &PathBuf) -> Result<Self> {
        let conn = Connection::open(path)?;
        
        // Enable WAL mode for concurrent reads
        conn.execute("PRAGMA journal_mode=WAL", [])?;
        conn.execute("PRAGMA synchronous=NORMAL", [])?;
        
        // Create tables
        conn.execute(
            "CREATE TABLE IF NOT EXISTS state (
                key TEXT PRIMARY KEY,
                value BLOB NOT NULL,
                version INTEGER NOT NULL DEFAULT 1,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                expires_at INTEGER
            )",
            [],
        )?;

        // Index for cleanup
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_expires 
             ON state(expires_at) 
             WHERE expires_at IS NOT NULL",
            [],
        )?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    fn store(&self, key: &str, value: &[u8], ttl: Option<u64>) -> Result<u64> {
        let conn = self.conn.lock().unwrap();
        let now = Self::now();
        let expires = ttl.map(|secs| now + secs);

        conn.execute(
            "INSERT INTO state (key, value, version, created_at, updated_at, expires_at)
             VALUES (?1, ?2, 1, ?3, ?3, ?4)
             ON CONFLICT(key) DO UPDATE SET
                value = excluded.value,
                version = version + 1,
                updated_at = excluded.updated_at,
                expires_at = excluded.expires_at",
            params![key, value, now, expires],
        )?;

        let version: u64 = conn.query_row(
            "SELECT version FROM state WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )?;

        Ok(version)
    }

    fn fetch(&self, key: &str) -> Result<Option<StateEntry>> {
        let conn = self.conn.lock().unwrap();
        let now = Self::now();

        let result = conn.query_row(
            "SELECT key, value, version, created_at 
             FROM state 
             WHERE key = ?1 AND (expires_at IS NULL OR expires_at > ?2)",
            params![key, now],
            |row| {
                Ok(StateEntry {
                    key: row.get(0)?,
                    value: row.get(1)?,
                    version: row.get(2)?,
                    timestamp: row.get(3)?,
                })
            },
        );

        match result {
            Ok(entry) => Ok(Some(entry)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn cleanup_expired(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let now = Self::now();
        
        let deleted = conn.execute(
            "DELETE FROM state WHERE expires_at IS NOT NULL AND expires_at < ?1",
            params![now],
        )?;

        Ok(deleted)
    }

    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }
}

#[service]
#[derive(Clone)]
struct StateManager {
    db: Arc<StateDb>,
}

#[handler]
impl StateManager {
    async fn store(&self, req: StoreRequest) -> Result<u64> {
        self.db.store(&req.key, &req.value, req.ttl_secs)
    }

    async fn fetch(&self, req: FetchRequest) -> Result<Option<StateEntry>> {
        self.db.fetch(&req.key)
    }

    async fn vacuum(&self) -> Result<u64> {
        Ok(self.db.cleanup_expired()? as u64)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();

    let home = dirs::home_dir().unwrap();
    let db_path = home.join(".cell/state.db");
    
    let db = StateDb::new(&db_path)?;

    // Background cleanup task
    let db_clone = Arc::new(db);
    let cleanup_db = db_clone.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            if let Ok(count) = cleanup_db.cleanup_expired() {
                if count > 0 {
                    tracing::info!("Cleaned up {} expired entries", count);
                }
            }
        }
    });

    let service = StateManager { db: db_clone };
    service.serve("state-manager").await
}