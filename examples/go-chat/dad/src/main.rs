use anyhow::Result;
use cell_sdk::{Membrane, protein};

// We redefine it here for the Rust macro to work as usual
#[protein(class = "DadMsg")]
pub struct DadMsg {
    pub a: u64,
    pub b: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("[Dad] Online. FP: {:x}", DadMsg::SCHEMA_FINGERPRINT);

    Membrane::bind("dad", |vesicle| async move {
        // Zero-copy deserialize
        let archived = cell_sdk::rkyv::check_archived_root::<DadMsg>(vesicle.as_slice()).unwrap();
        
        println!("[Dad] Received: {} + {}", archived.a, archived.b);
        
        let result = DadMsg {
            a: archived.a + archived.b,
            b: 0
        };
        
        let bytes = cell_sdk::rkyv::to_bytes::<_, 16>(&result)?.into_vec();
        Ok(cell_sdk::vesicle::Vesicle::wrap(bytes))
    }).await
}