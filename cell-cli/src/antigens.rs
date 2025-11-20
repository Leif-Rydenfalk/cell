use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use snow::Builder;
use std::fs;
use std::path::PathBuf;

pub struct Antigens {
    pub keypair: snow::Keypair,
    pub public_key_str: String,
}

impl Antigens {
    pub fn load_or_create() -> Result<Self> {
        // Use standard linux paths or local hidden dir
        let home = dirs::home_dir().context("Failed to determine home directory")?;
        let cell_home = home.join(".cell");
        if !cell_home.exists() {
            fs::create_dir_all(&cell_home)?;
        }

        let key_path = cell_home.join("node_identity");

        if key_path.exists() {
            Self::load(&key_path)
        } else {
            Self::generate(&key_path)
        }
    }

    fn load(path: &PathBuf) -> Result<Self> {
        let content = fs::read_to_string(path).context("Failed to read identity file")?;
        let parts: Vec<&str> = content.trim().split(':').collect();

        if parts.len() != 2 {
            anyhow::bail!("Identity file corrupted. Expected format PUBLIC:PRIVATE");
        }

        let public = B64
            .decode(parts[0])
            .context("Failed to decode public key")?;
        let private = B64
            .decode(parts[1])
            .context("Failed to decode private key")?;

        Ok(Self {
            keypair: snow::Keypair { public, private },
            public_key_str: parts[0].to_string(),
        })
    }

    fn generate(path: &PathBuf) -> Result<Self> {
        // Protocol: Noise_XX_25519_ChaChaPoly_BLAKE2s
        // XX Pattern: Mutual authentication. Both sides exchange static keys.
        let builder = Builder::new("Noise_XX_25519_ChaChaPoly_BLAKE2s".parse()?);
        let keypair = builder.generate_keypair()?;

        let pub_b64 = B64.encode(&keypair.public);
        let priv_b64 = B64.encode(&keypair.private);

        // Atomic write could be added here for safety, standard write for now
        fs::write(path, format!("{}:{}", pub_b64, priv_b64))?;

        // Strict permissions: 600 (Read/Write by owner only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }

        eprintln!("[INFO] Identity initialized. Node ID: {}", pub_b64);
        Ok(Self {
            keypair,
            public_key_str: pub_b64,
        })
    }
}
