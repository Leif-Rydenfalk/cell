use anyhow::Result;
use cell_sdk as cell;
use cell_sdk::registry::{InstanceInfo, InstanceRegistry};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

#[cell::service]
#[derive(Clone)]
struct CellGitService {
    storage_root: PathBuf,
    // In-memory cache of instance registries (expires after 60s)
    instance_cache: Arc<RwLock<HashMap<String, (InstanceRegistry, std::time::Instant)>>>,
}

#[cell::handler]
impl CellGitService {
    /// Fetch a file from a repository at a specific ref
    async fn fetch_file(&self, repo: String, ref_name: String, path: String) -> Result<Vec<u8>> {
        let repo_path = self.storage_root.join("repos").join(&repo);

        // Blocking Git operations need to run on a blocking thread to not starve Tokio
        let path_clone = path.clone();
        let res = tokio::task::spawn_blocking(move || -> Result<Vec<u8>> {
            let repo = git2::Repository::open(&repo_path)?;
            let obj = repo.revparse_single(&ref_name)?;
            let commit = obj.peel_to_commit()?;
            let tree = commit.tree()?;
            let entry = tree.get_path(std::path::Path::new(&path_clone))?;
            let blob = repo.find_blob(entry.id())?;
            Ok(blob.content().to_vec())
        })
        .await??;

        Ok(res)
    }

    /// Get the manifest (Cell.json) for a cell at a specific version
    async fn get_manifest(&self, repo: String, tag: String) -> Result<Vec<u8>> {
        // We reuse fetch_file logic
        self.fetch_file(repo, tag, "Cell.json".to_string()).await
    }

    /// Get live instances for a cell (from mutable store, not git)
    async fn get_instances(&self, repo: String) -> Result<Vec<u8>> {
        let cache_key = repo.clone();

        // Check cache first (60s TTL)
        {
            let cache = self.instance_cache.read().await;
            if let Some((registry, timestamp)) = cache.get(&cache_key) {
                if timestamp.elapsed().as_secs() < 60 {
                    return Ok(serde_json::to_vec(registry)?);
                }
            }
        }

        // Cache miss or expired - read from disk
        let instances_path = self
            .storage_root
            .join("instances")
            .join(&repo)
            .join("instances.json");

        if !instances_path.exists() {
            let empty = InstanceRegistry {
                cell_name: repo.clone(),
                version: "unknown".into(),
                instances: vec![],
                updated_at: chrono::Utc::now().to_rfc3339(),
            };
            return Ok(serde_json::to_vec(&empty)?);
        }

        let data = tokio::fs::read(&instances_path).await?;
        let registry: InstanceRegistry = serde_json::from_slice(&data)?;

        // Update cache
        {
            let mut cache = self.instance_cache.write().await;
            cache.insert(cache_key, (registry.clone(), std::time::Instant::now()));
        }

        Ok(serde_json::to_vec(&registry)?)
    }

    /// Announce a running instance (heartbeat)
    async fn announce_instance(&self, repo: String, instance: InstanceInfo) -> Result<()> {
        let instances_path = self
            .storage_root
            .join("instances")
            .join(&repo)
            .join("instances.json");

        if let Some(p) = instances_path.parent() {
            tokio::fs::create_dir_all(p).await?;
        }

        // Read existing registry (naive read-modify-write, locking needed for high concurency later)
        let mut registry = if instances_path.exists() {
            let data = tokio::fs::read(&instances_path).await?;
            serde_json::from_slice::<InstanceRegistry>(&data)?
        } else {
            InstanceRegistry {
                cell_name: repo.clone(),
                version: "unknown".into(),
                instances: vec![],
                updated_at: chrono::Utc::now().to_rfc3339(),
            }
        };

        // Remove stale instances (no heartbeat in 30 seconds)
        let now = chrono::Utc::now();
        registry.instances.retain(|i| {
            if let Ok(last) = chrono::DateTime::parse_from_rfc3339(&i.last_heartbeat) {
                (now - last).num_seconds() < 30
            } else {
                false
            }
        });

        // Update or insert this instance
        if let Some(existing) = registry
            .instances
            .iter_mut()
            .find(|i| i.node_id == instance.node_id)
        {
            *existing = instance;
        } else {
            registry.instances.push(instance);
        }

        registry.updated_at = now.to_rfc3339();

        // Write back atomically
        let tmp_path = instances_path.with_extension("tmp");
        tokio::fs::write(&tmp_path, serde_json::to_vec_pretty(&registry)?).await?;
        tokio::fs::rename(&tmp_path, &instances_path).await?;

        // Invalidate cache
        {
            let mut cache = self.instance_cache.write().await;
            cache.remove(&repo);
        }

        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let storage_root = dirs::home_dir().expect("No home dir").join(".cell/git");

    tokio::fs::create_dir_all(&storage_root).await?;

    let service = CellGitService {
        storage_root: storage_root.clone(),
        instance_cache: Arc::new(RwLock::new(HashMap::new())),
    };

    println!("[CellGit] Storage: {:?}", storage_root);
    println!("[CellGit] Fingerprint: 0x{:x}", CellGitService::SCHEMA_FINGERPRINT);
    
    // Serve on standard name
    service.serve("cell-git").await
}