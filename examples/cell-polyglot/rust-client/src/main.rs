use anyhow::Result;
use cell_sdk::vesicle::Vesicle;
use cell_sdk::Synapse;
use serde::{Deserialize, Serialize};

// Standard Rust Structs derived with Serde
// Note: We use standard serde_json, NOT rkyv here!
#[derive(Serialize, Debug)]
struct TextRequest {
    text: String,
}

#[derive(Deserialize, Debug)]
struct TextResponse {
    original: String,
    reversed: String,
    uppercase: String,
    processed_by: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("Rust Client Starting...");
    
    // 1. Create Data
    let req = TextRequest {
        text: "Polyglot Cells are Cool".to_string(),
    };

    // 2. Serialize to JSON Bytes
    let payload = serde_json::to_vec(&req)?;
    let v_out = Vesicle::wrap(payload);

    // 3. Connect to Python Cell
    let mut synapse = Synapse::grow("py-worker")?;
    
    // 4. Fire
    println!("Sending to Python: {:?}", req);
    let v_in = synapse.fire(v_out)?;

    // 5. Deserialize Response
    let resp: TextResponse = serde_json::from_slice(v_in.as_slice())?;
    
    println!("Response from Python:");
    println!("{:#?}", resp);

    Ok(())
}