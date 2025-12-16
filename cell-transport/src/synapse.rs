// cell-transport/src/synapse.rs
use crate::resolve_socket_dir;
use crate::response::Response;
use anyhow::{Context, Result};
use cell_core::{channel, RouterDescriptor, VesicleHeader};
use cell_model::rkyv::ser::serializers::AllocSerializer;
use rkyv::Serialize;
use tokio::fs::OpenOptions;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub struct Synapse {
    my_id: u64,
}

impl Synapse {
    pub async fn new(name: &str) -> Result<Self> {
        let hash = blake3::hash(name.as_bytes());
        let my_id = u64::from_le_bytes(hash.as_bytes()[..8].try_into().unwrap());
        Ok(Self { my_id })
    }

    pub async fn fire<'a, Req>(&mut self, target: &str, req: &Req) -> Result<Response<'a, ()>>
    // Simplified resp type
    where
        Req: Serialize<AllocSerializer<1024>>,
    {
        // 1. Identify Target
        let t_hash = blake3::hash(target.as_bytes());
        let t_id = u64::from_le_bytes(t_hash.as_bytes()[..8].try_into().unwrap());

        // 2. Find the Router File
        // Path: .cell/routers/<ID>.router
        let root = resolve_socket_dir();
        let router_path = root.join("routers").join(format!("{:016x}.router", t_id));

        if !router_path.exists() {
            return Err(anyhow::anyhow!("No route to '{}' ({:016x})", target, t_id).into());
        }

        // 3. Read the Pipe Name from the file
        let desc_bytes = tokio::fs::read(&router_path).await?;
        let desc = RouterDescriptor::from_bytes(&desc_bytes).context("Corrupt router file")?;

        let pipe_str = std::str::from_utf8(&desc.pipe_name)?.trim_matches(char::from(0));

        let pipe_path = root.join("pipes").join(pipe_str);

        // 4. Write to the Pipe
        let mut tx = OpenOptions::new()
            .write(true)
            .open(&pipe_path) // In real world, use .append(true) for atomic writes on some OS
            .await?;

        let req_bytes = rkyv::to_bytes::<_, 1024>(req)?.into_vec();

        let header = VesicleHeader {
            target_id: t_id,
            source_id: self.my_id,
            ttl: 64,
            _pad: [0; 7],
        };

        // Frame: [Len:4][Header:24][Channel:1][Payload:N]
        let total_len = 24 + 1 + req_bytes.len();

        tx.write_all(&(total_len as u32).to_le_bytes()).await?;

        let h_bytes: [u8; 24] = unsafe { std::mem::transmute(header) };
        tx.write_all(&h_bytes).await?;

        tx.write_u8(channel::APP).await?;
        tx.write_all(&req_bytes).await?;

        // 5. Receive Reply
        // (Omitted: Requires reading from .cell/pipes/<my_id>_in)
        Ok(Response::Owned(vec![]))
    }
}
