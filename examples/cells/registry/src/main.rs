// cells/registry/src/main.rs
// SPDX-License-Identifier: MIT
// Git-as-registry with signature verification (decentralized package manager)

use cell_sdk::*;
use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

// === REGISTRY PROTOCOL ===

#[protein]
pub struct Package {
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    pub git_url: String,
    pub commit_hash: String,
    pub signature: Vec<u8>,
}

#[protein]
pub struct PublishRequest {
    pub package: Package,
    pub source_tarball: Vec<u8>,
    pub signing_key: Vec<u8>,
}

#[protein]
pub struct SearchQuery {
    pub query: String,
    pub limit: u32,
}

#[protein]
pub struct SearchResult {
    pub packages: Vec<PackageMetadata>,
}

#[protein]
pub struct PackageMetadata {
    pub name: String,
    pub version: String,
    pub description: String,
    pub downloads: u64,
    pub stars: u32,
}

#[protein]
pub struct InstallRequest {
    pub name: String,
    pub version: String,
    pub verify_signature: bool,
}

#[protein]
pub struct InstallResult {
    pub success: bool,
    pub installed_path: String,
    pub verified: bool,
}

#[protein]
pub struct TrustKey {
    pub author: String,
    pub public_key: Vec<u8>,
}

// === REGISTRY SERVICE ===

pub struct RegistryService {
    repo_root: PathBuf,
    packages: Arc<RwLock<HashMap<String, Vec<Package>>>>,
    trusted_keys: Arc<RwLock<HashMap<String, Vec<u8>>>>,
    stats: Arc<RwLock<PackageStats>>,
}

struct PackageStats {
    downloads: HashMap<String, u64>,
    stars: HashMap<String, u32>,
}

impl RegistryService {
    pub fn new(repo_root: PathBuf) -> Self {
        Self {
            repo_root,
            packages: Arc::new(RwLock::new(HashMap::new())),
            trusted_keys: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(RwLock::new(PackageStats {
                downloads: HashMap::new(),
                stars: HashMap::new(),
            })),
        }
    }

    async fn verify_signature(&self, package: &Package) -> Result<bool> {
        let keys = self.trusted_keys.read().await;
        
        if let Some(_public_key) = keys.get(&package.author) {
            // In real impl: use ed25519 or RSA signature verification
            let _payload = format!("{}:{}:{}", 
                package.name, package.version, package.commit_hash);
            
            // Placeholder verification
            let valid = package.signature.len() == 64;
            Ok(valid)
        } else {
            Ok(false)
        }
    }

    async fn clone_or_pull_repo(&self, git_url: &str, commit_hash: &str) -> Result<PathBuf> {
        use std::process::Command;
        
        let repo_name = git_url.split('/').last()
            .ok_or_else(|| anyhow::anyhow!("Invalid git URL"))?
            .trim_end_matches(".git");
        
        let repo_path = self.repo_root.join(repo_name);
        
        if !repo_path.exists() {
            // Clone
            let status = Command::new("git")
                .args(&["clone", "--depth", "1", git_url])
                .arg(&repo_path)
                .status()?;
            
            if !status.success() {
                anyhow::bail!("Git clone failed");
            }
        }
        
        // Checkout specific commit
        let status = Command::new("git")
            .args(&["checkout", commit_hash])
            .current_dir(&repo_path)
            .status()?;
        
        if !status.success() {
            anyhow::bail!("Git checkout failed");
        }
        
        Ok(repo_path)
    }

    async fn build_package(&self, source_path: &PathBuf) -> Result<PathBuf> {
        use std::process::Command;
        
        let status = Command::new("cargo")
            .args(&["build", "--release"])
            .current_dir(source_path)
            .status()?;
        
        if !status.success() {
            anyhow::bail!("Build failed");
        }
        
        Ok(source_path.join("target/release"))
    }
}

#[handler]
impl RegistryService {
    pub async fn publish(&self, req: PublishRequest) -> Result<bool> {
        // Verify signature
        if !self.verify_signature(&req.package).await? {
            anyhow::bail!("Invalid signature");
        }
        
        // Add to registry
        self.packages.write().await
            .entry(req.package.name.clone())
            .or_insert_with(Vec::new)
            .push(req.package.clone());
        
        // Commit to git repo (in real impl)
        println!("[Registry] Published {}@{}", req.package.name, req.package.version);
        
        Ok(true)
    }

    pub async fn search(&self, query: SearchQuery) -> Result<SearchResult> {
        let packages_map = self.packages.read().await;
        let stats = self.stats.read().await;
        
        let mut results = Vec::new();
        
        for (name, versions) in packages_map.iter() {
            if name.contains(&query.query) {
                if let Some(latest) = versions.last() {
                    results.push(PackageMetadata {
                        name: name.clone(),
                        version: latest.version.clone(),
                        description: latest.description.clone(),
                        downloads: stats.downloads.get(name).copied().unwrap_or(0),
                        stars: stats.stars.get(name).copied().unwrap_or(0),
                    });
                }
            }
        }
        
        results.sort_by_key(|p| std::cmp::Reverse(p.downloads));
        results.truncate(query.limit as usize);
        
        Ok(SearchResult { packages: results })
    }

    pub async fn install(&self, req: InstallRequest) -> Result<InstallResult> {
        let packages = self.packages.read().await;
        
        let package = packages.get(&req.name)
            .and_then(|versions| {
                versions.iter().find(|p| p.version == req.version)
            })
            .ok_or_else(|| anyhow::anyhow!("Package not found"))?;
        
        // Verify if requested
        let verified = if req.verify_signature {
            self.verify_signature(package).await?
        } else {
            false
        };
        
        // Clone repo
        let source_path = self.clone_or_pull_repo(&package.git_url, &package.commit_hash).await?;
        
        // Build
        let binary_path = self.build_package(&source_path).await?;
        
        // Update stats
        {
            let mut stats = self.stats.write().await;
            *stats.downloads.entry(req.name.clone()).or_insert(0) += 1;
        }
        
        Ok(InstallResult {
            success: true,
            installed_path: binary_path.display().to_string(),
            verified,
        })
    }

    pub async fn trust(&self, key: TrustKey) -> Result<bool> {
        self.trusted_keys.write().await.insert(key.author, key.public_key);
        Ok(true)
    }

    pub async fn star(&self, package_name: String) -> Result<u32> {
        let mut stats = self.stats.write().await;
        let count = stats.stars.entry(package_name).or_insert(0);
        *count += 1;
        Ok(*count)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let repo_root = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("No home dir"))?
        .join(".cell/registry");
    
    tokio::fs::create_dir_all(&repo_root).await?;
    
    let registry = RegistryService::new(repo_root);
    
    println!("[Registry] Git-based package registry active");
    registry.serve("registry").await
}