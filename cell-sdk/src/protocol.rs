use crate::protein;

#[protein]
pub enum MitosisRequest {
    Spawn { cell_name: String },
}

#[protein]
pub enum MitosisResponse {
    Ok { socket_path: String },
    Denied { reason: String },
}
