use anyhow::Result;
use cell_sdk::vesicle::Vesicle;
use cell_sdk::{Synapse, protein};
// Note: We don't need to import serde::Serialize manually anymore, 
// #[protein] does it, but we do need serde_json for the manual encoding.

#[protein]
struct TextRequest {
    text: String,
}

#[protein]
struct TextResponse {
    original: String,
    reversed: String,
    uppercase: String,
    processed_by: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("Rust Client Starting...");
    
    // 1. Create Data (Protein)
    let req = TextRequest {
        text: "Polyglot Cells are Cool".to_string(),
    };

    // 2. Serialize to JSON Bytes (For Python compatibility)
    // Since #[protein] derived Serde::Serialize, this works out of the box.
    let payload = serde_json::to_vec(&req)?;
    let v_out = Vesicle::wrap(payload);

    // 3. Connect to Python Cell
    let mut synapse = Synapse::grow("py-worker")?;
    
    // 4. Fire
    println!("Sending to Python: {:?}", req);
    let v_in = synapse.fire(v_out)?;

    // 5. Deserialize Response
    // #[protein] derived Serde::Deserialize too.
    let resp: TextResponse = serde_json::from_slice(v_in.as_slice())?;
    
    println!("Response from Python:");
    println!("{:#?}", resp);

    Ok(())
}