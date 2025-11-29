use crate::protein;
use serde::{Deserialize, Serialize}; // Ensure protein macro is available

// --- INTROSPECTION PROTOCOL ---
pub const GENOME_REQUEST: &[u8] = b"__CELL_GENOME_REQUEST__";

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CellGenome {
    pub name: String,
    pub fingerprint: u64,
    pub methods: Vec<MethodSchema>,
    pub types: Vec<TypeSchema>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MethodSchema {
    pub name: String,
    pub inputs: Vec<(String, TypeRef)>,
    pub output: TypeRef,
    pub is_stream: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TypeSchema {
    pub name: String,
    pub kind: TypeKind,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum TypeKind {
    Struct {
        fields: Vec<(String, TypeRef)>,
    },
    Enum {
        variants: Vec<(String, Vec<TypeRef>)>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum TypeRef {
    Named(String),
    Primitive(Primitive),
    Vec(Box<TypeRef>),
    Option(Box<TypeRef>),
    Unknown,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Primitive {
    String,
    U8,
    U16,
    U32,
    U64,
    I8,
    I16,
    I32,
    I64,
    F32,
    F64,
    Bool,
}

// --- LIFECYCLE PROTOCOL (Mitosis) ---

#[protein]
pub enum MitosisRequest {
    Spawn { cell_name: String },
}

#[protein]
pub enum MitosisResponse {
    Ok { socket_path: String },
    Denied { reason: String },
}
