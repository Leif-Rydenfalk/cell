// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use crate::config::CellInitConfig;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const GENOME_REQUEST: &[u8] = b"__CELL_GENOME_REQUEST__";
pub const SHM_UPGRADE_REQUEST: &[u8] = b"__SHM_UPGRADE_REQUEST__";
pub const SHM_UPGRADE_ACK: &[u8] = b"__SHM_UPGRADE_ACK__";

pub const GAP_JUNCTION_FD: i32 = 3;

// ... (Existing CellGenome structs remain unchanged) ...
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CellGenome {
    pub name: String,
    pub fingerprint: u64,
    pub methods: Vec<MethodSchema>,
    pub types: Vec<TypeSchema>,
    pub macros: Vec<MacroSchema>,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MethodSchema {
    pub name: String,
    pub inputs: Vec<(String, TypeRef)>,
    pub output: TypeRef,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TypeSchema {
    pub name: String,
    pub kind: TypeKind,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum TypeKind {
    Struct { fields: Vec<(String, TypeRef)> },
    Enum { variants: Vec<(String, Vec<TypeRef>)> },
}
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MacroSchema {
    pub name: String,
    pub kind: MacroKind,
    pub source: String,
    pub dependencies: Vec<String>,
}
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum MacroKind { Declarative, Attribute, Derive, Function }
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum TypeRef { Named(String), Primitive(Primitive), Vec(Box<TypeRef>), Option(Box<TypeRef>), Result(Box<TypeRef>, Box<TypeRef>), Unit, Unknown }
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub enum Primitive { String, U8, U16, U32, U64, I8, I16, I32, I64, F32, F64, Bool }

#[derive(Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug)]
#[archive(check_bytes)]
pub enum MitosisRequest {
    Spawn { cell_name: String, config: Option<CellInitConfig> },
    Test { target_cell: String, filter: Option<String> },
}

#[derive(Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug)]
#[archive(check_bytes)]
pub enum MitosisResponse {
    Ok { socket_path: String },
    Denied { reason: String },
}

// --- MESH PROTOCOL ---

#[derive(Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug)]
#[archive(check_bytes)]
pub enum MeshRequest {
    ResolveDependencies { cell_name: String, dependencies: Vec<String> },
    ReportHealth { cell_name: String, healthy: bool },
    // NEW: Request the full graph to perform GC analysis
    GetFullGraph,
}

#[derive(Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug)]
#[archive(check_bytes)]
pub enum MeshResponse {
    DependencyMapping { cell_name: String, socket_paths: HashMap<String, String> },
    Ack,
    // NEW: Return the graph (Consumer -> [Providers])
    FullGraph(HashMap<String, Vec<String>>),
    Error { message: String },
}

// --- NUCLEUS PROTOCOL EXTENSION (Handled via generic RPC usually, but defining here for clarity) ---
// Note: Nucleus usually uses the `handler!` macro which generates specific enums. 
// We will update main.rs in Nucleus to include the Vacuum method.

#[derive(Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug)]
#[archive(check_bytes)]
pub enum TestEvent {
    Log(String),
    CaseStarted(String),
    CaseFinished { name: String, success: bool, duration_ms: u64 },
    SuiteFinished { total: u32, passed: u32, failed: u32 },
    Error(String),
}

#[derive(Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug)]
#[archive(check_bytes)]
pub enum MitosisSignal {
    Prophase,
    RequestIdentity,
    Prometaphase { socket_path: String },
    Cytokinesis,
    Apoptosis { reason: String },
    Necrosis,
}

#[derive(Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug)]
#[archive(check_bytes)]
pub enum MitosisControl {
    InjectIdentity(CellInitConfig),
    Terminate,
}