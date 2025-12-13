// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use crate::transport::UnixTransport;
use cell_core::{Transport, CellError, channel};
use anyhow::{bail, Result};
use rkyv::ser::serializers::AllocSerializer;
use rkyv::{Archive, Serialize};
use tokio::net::UnixStream;
use std::time::Duration;

pub struct Synapse {
    transport: Box<dyn Transport>,
}

impl Synapse {
    pub async fn grow(cell_name: &str) -> Result<Self> {
        let socket_dir = crate::resolve_socket_dir();
        let socket_path = socket_dir.join(format!("{}.sock", cell_name));

        // If socket doesn't exist, spawn the cell
        if !socket_path.exists() {
            Self::spawn_cell(cell_name).await?;
            
            // Wait for socket to appear
            for _ in 0..50 {
                if socket_path.exists() { break; }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            
            if !socket_path.exists() {
                bail!("Cell '{}' failed to start", cell_name);
            }
        }

        let stream = UnixStream::connect(&socket_path).await?;
        Ok(Self {
            transport: Box::new(UnixTransport::new(stream)),
        })
    }

    async fn spawn_cell(cell_name: &str) -> Result<()> {
        let binary = Self::find_or_compile_binary(cell_name)?;
        
        std::process::Command::new(binary)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::inherit())
            .spawn()?;
        
        Ok(())
    }

    fn find_or_compile_binary(cell_name: &str) -> Result<std::path::PathBuf> {
        let home = dirs::home_dir().unwrap();
        let bin_dir = home.join(".cell/bin");
        let binary_path = bin_dir.join(cell_name);

        // If binary exists and is fresh, use it
        if binary_path.exists() {
            return Ok(binary_path);
        }

        // Compile it
        std::fs::create_dir_all(&bin_dir)?;
        
        let source_path = Self::find_source(cell_name)?;
        
        tracing::info!("[Synapse] Compiling {}...", cell_name);
        
        let status = std::process::Command::new("cargo")
            .arg("build")
            .arg("--release")
            .arg("--manifest-path")
            .arg(source_path.join("Cargo.toml"))
            .status()?;

        if !status.success() {
            bail!("Failed to compile {}", cell_name);
        }

        // Find the built binary
        let target_binary = source_path
            .join("target/release")
            .join(cell_name);

        if !target_binary.exists() {
            bail!("Binary not found after compilation: {:?}", target_binary);
        }

        // Copy to bin dir
        std::fs::copy(&target_binary, &binary_path)?;
        
        Ok(binary_path)
    }

    fn find_source(cell_name: &str) -> Result<std::path::PathBuf> {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
            .unwrap_or_else(|_| ".".to_string());
        let current = std::path::Path::new(&manifest_dir);

        let search_paths = vec![
            current.join("cells").join(cell_name),
            current.join("../cells").join(cell_name),
            current.join("../../cells").join(cell_name),
            current.join("examples/cell-market-bench/cells").join(cell_name),
            current.join("../examples/cell-market-bench/cells").join(cell_name),
            current.join("../../examples/cell-market-bench/cells").join(cell_name),
        ];

        for path in search_paths {
            if path.join("Cargo.toml").exists() {
                return Ok(path);
            }
        }

        bail!("Could not find source for cell '{}'", cell_name);
    }

    pub async fn fire<'a, Req, Resp>(&'a mut self, request: &Req) -> Result<Resp, CellError>
    where
        Req: Serialize<AllocSerializer<1024>>,
        Resp: Archive + 'a,
        Resp::Archived: rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'static>> + 'static + rkyv::Deserialize<Resp, rkyv::de::deserializers::SharedDeserializeMap>,
    {
        let req_bytes = rkyv::to_bytes::<_, 1024>(request)
            .map_err(|_| CellError::SerializationFailure)?
            .into_vec();
        
        let mut frame = Vec::with_capacity(1 + req_bytes.len());
        frame.push(channel::APP);
        frame.extend_from_slice(&req_bytes);

        let resp_bytes = self.transport.call(&frame).await?;
        
        let archived = rkyv::check_archived_root::<Resp>(&resp_bytes)
            .map_err(|_| CellError::SerializationFailure)?;
        
        Ok(archived.deserialize(&mut rkyv::de::deserializers::SharedDeserializeMap::new())
            .map_err(|_| CellError::SerializationFailure)?)
    }
}