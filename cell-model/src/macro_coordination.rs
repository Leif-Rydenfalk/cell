// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use rkyv::{Archive, Serialize as RkyvSerialize, Deserialize as RkyvDeserialize};
use serde::{Serialize as SerdeSerialize, Deserialize as SerdeDeserialize};
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Archive, RkyvSerialize, RkyvDeserialize, SerdeSerialize, SerdeDeserialize, Debug, Clone)]
#[archive(check_bytes)]
pub struct MacroInfo {
    pub name: String,
    pub kind: MacroKind,
    pub description: String,
    pub dependencies: Vec<String>,
}

#[derive(Archive, RkyvSerialize, RkyvDeserialize, SerdeSerialize, SerdeDeserialize, Debug, Clone, PartialEq)]
#[archive(check_bytes)]
pub enum MacroKind {
    Attribute,
    Derive,
    Function,
}

#[derive(Archive, RkyvSerialize, RkyvDeserialize, SerdeSerialize, SerdeDeserialize, Debug, Clone)]
#[archive(check_bytes)]
pub struct ExpansionContext {
    pub struct_name: String,
    pub fields: Vec<(String, String)>, // (field_name, type_name)
    pub attributes: Vec<String>,
    pub other_cells: Vec<String>, // Other cells involved in this expansion
}

#[derive(Archive, RkyvSerialize, RkyvDeserialize, SerdeSerialize, SerdeDeserialize, Debug, Clone)]
#[archive(check_bytes)]
pub enum MacroCoordinationRequest {
    WhatMacrosDoYouProvide,
    GetMacroInfo { name: String },
    CoordinateExpansion {
        macro_name: String,
        context: ExpansionContext,
    },
    QueryOtherCell {
        target_cell: String,
        query: String,
    },
}

#[derive(Archive, RkyvSerialize, RkyvDeserialize, SerdeSerialize, SerdeDeserialize, Debug, Clone)]
#[archive(check_bytes)]
pub enum MacroCoordinationResponse {
    Macros { macros: Vec<MacroInfo> },
    MacroInfo { info: MacroInfo },
    GeneratedCode { code: String },
    QueryResult { result: String },
    Error { message: String },
}