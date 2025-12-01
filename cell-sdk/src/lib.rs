extern crate self as cell_sdk;

pub mod capsid;
pub mod membrane;
pub mod pheromones;
pub mod protocol;
pub mod ribosome;
pub mod root;
pub mod shm;
pub mod synapse;
pub mod vesicle;

// Re-exports for ease of use
pub use cell_macros::{cell_remote, handler, protein, service};
pub use membrane::Membrane;
pub use root::MyceliumRoot;
pub use shm::ShmClient;
pub use synapse::Synapse;
pub use vesicle::Vesicle;

// Re-export dependencies used by macros
pub use rkyv;
pub use serde;

// Helper for macros
pub use membrane::resolve_socket_dir;
