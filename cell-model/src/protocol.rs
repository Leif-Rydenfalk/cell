// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

pub const GENOME_REQUEST: &[u8] = b"__CELL_GENOME_REQUEST__";
pub const SHM_UPGRADE_REQUEST: &[u8] = b"__SHM_UPGRADE_REQUEST__";
pub const SHM_UPGRADE_ACK: &[u8] = b"__SHM_UPGRADE_ACK__";

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

#[derive(Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[archive(check_bytes)]
pub enum MitosisRequest {
    Spawn { cell_name: String },
}

#[derive(Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[archive(check_bytes)]
pub enum MitosisResponse {
    Ok { socket_path: String },
    Denied { reason: String },
}