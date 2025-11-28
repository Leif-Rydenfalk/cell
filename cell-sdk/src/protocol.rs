use crate::rkyv;
use cell_macros::protein;

#[protein]
pub enum MitosisRequest {
    /// "I need this cell to exist."
    Spawn { cell_name: String },
}

#[protein]
pub enum MitosisResponse {
    /// "It is done. Connect here."
    Ok { socket_path: String },
    /// "I'm sorry, Dave. I can't do that."
    Denied { reason: String },
}
