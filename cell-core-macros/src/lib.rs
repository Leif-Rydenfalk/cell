// cell-core-macros/src/lib.rs
use alloc::string::String;
use alloc::vec::Vec;
use rkyv::{Archive, Deserialize as RkyvDeserialize, Serialize as RkyvSerialize};
use serde::{Deserialize as SerdeDeserialize, Serialize as SerdeSerialize};

#[derive(
    Archive, RkyvSerialize, RkyvDeserialize, SerdeSerialize, SerdeDeserialize, Debug, Clone,
)]
#[archive(check_bytes)]
pub struct MacroInfo {
    pub name: String,
    pub kind: MacroKind,
    pub description: String,
    pub dependencies: Vec<String>,
}

#[derive(
    Archive,
    RkyvSerialize,
    RkyvDeserialize,
    SerdeSerialize,
    SerdeDeserialize,
    Debug,
    Clone,
    PartialEq,
)]
#[archive(check_bytes)]
pub enum MacroKind {
    Attribute,
    Derive,
    Function,
}

#[derive(
    Archive, RkyvSerialize, RkyvDeserialize, SerdeSerialize, SerdeDeserialize, Debug, Clone,
)]
#[archive(check_bytes)]
pub struct ExpansionContext {
    pub struct_name: String,
    pub fields: Vec<(String, String)>,
    pub attributes: Vec<String>,
    pub other_cells: Vec<String>,
}

#[derive(
    Archive, RkyvSerialize, RkyvDeserialize, SerdeSerialize, SerdeDeserialize, Debug, Clone,
)]
#[archive(check_bytes)]
pub enum MacroCoordinationRequest {
    WhatMacrosDoYouProvide,
    GetMacroInfo {
        name: String,
    },
    CoordinateExpansion {
        macro_name: String,
        context: ExpansionContext,
    },
    QueryOtherCell {
        target_cell: String,
        query: String,
    },
}

#[derive(
    Archive, RkyvSerialize, RkyvDeserialize, SerdeSerialize, SerdeDeserialize, Debug, Clone,
)]
#[archive(check_bytes)]
pub enum MacroCoordinationResponse {
    Macros { macros: Vec<MacroInfo> },
    MacroInfo { info: MacroInfo },
    GeneratedCode { code: String },
    QueryResult { result: String },
    Error { message: String },
}
