// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};
use crate::config::CellInitConfig;

pub const GENOME_REQUEST: &[u8] = b"__CELL_GENOME_REQUEST__";
pub const SHM_UPGRADE_REQUEST: &[u8] = b"__SHM_UPGRADE_REQUEST__";
pub const SHM_UPGRADE_ACK: &[u8] = b"__SHM_UPGRADE_ACK__";

// The Gap Junction File Descriptor index
pub const GAP_JUNCTION_FD: i32 = 3;

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
    Struct {
        fields: Vec<(String, TypeRef)>,
    },
    Enum {
        variants: Vec<(String, Vec<TypeRef>)>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MacroSchema {
    pub name: String,
    pub kind: MacroKind,
    pub source: String,
    pub dependencies: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum MacroKind {
    Declarative,
    Attribute,
    Derive,
    Function,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum TypeRef {
    Named(String),
    Primitive(Primitive),
    Vec(Box<TypeRef>),
    Option(Box<TypeRef>),
    Result(Box<TypeRef>, Box<TypeRef>),
    Unit,
    Unknown,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
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

// Top-level Daemon Request
#[derive(Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug)]
#[archive(check_bytes)]
pub enum MitosisRequest {
    /// Spawn a standard long-lived Cell
    Spawn { 
        cell_name: String,
        config: Option<CellInitConfig>,
    },
    /// Run a test suite as an ephemeral Cell
    Test {
        target_cell: String,
        filter: Option<String>,
    }
}

#[derive(Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug)]
#[archive(check_bytes)]
pub enum MitosisResponse {
    Ok { socket_path: String },
    Denied { reason: String },
    // Test responses are streamed as TestEvent, not returned as a single MitosisResponse
}

/// Events streamed back from Hypervisor -> CLI during a test run
#[derive(Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug)]
#[archive(check_bytes)]
pub enum TestEvent {
    Log(String),
    CaseStarted(String),
    CaseFinished { name: String, success: bool, duration_ms: u64 },
    SuiteFinished { total: u32, passed: u32, failed: u32 },
    Error(String),
}

/// Signals sent from the Daughter Cell to the Progenitor (System/Hypervisor) via the Gap Junction.
#[derive(Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug)]
#[archive(check_bytes)]
pub enum MitosisSignal {
    /// "I am alive, but building my internal structures."
    Prophase,
    /// "I need my genetic sequence (Configuration)."
    RequestIdentity,
    /// "My membrane is bound at this address."
    Prometaphase { socket_path: String },
    /// "I am fully independent. Sever the connection."
    Cytokinesis,
    /// "I am dying cleanly."
    Apoptosis { reason: String },
    /// "I have sustained fatal trauma."
    Necrosis,
}

/// Control messages sent from the Progenitor to the Daughter via the Gap Junction.
#[derive(Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug)]
#[archive(check_bytes)]
pub enum MitosisControl {
    /// "Here is your genetic sequence."
    InjectIdentity(CellInitConfig),
    /// "Die."
    Terminate,
}