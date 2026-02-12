// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

//! Schema Registry Types
//!
//! This module provides the foundation for a robust, versioned schema system
//! that enables compile-time type sharing between cells.

use alloc::string::String;
use alloc::vec::Vec;
use rkyv::{Archive, Deserialize as RkyvDeserialize, Serialize as RkyvSerialize};
use serde::{Deserialize as SerdeDeserialize, Serialize as SerdeSerialize};

/// A versioned schema entry in the registry.
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
pub struct SchemaEntry {
    /// Schema name (usually the struct name)
    pub name: String,

    /// Semantic version of the schema
    pub version: SchemaVersion,

    /// Field definitions
    pub fields: Vec<FieldDef>,

    /// Additional metadata (documentation, constraints, etc.)
    pub metadata: SchemaMetadata,

    /// Hash of the source code that generated this schema
    pub source_hash: String,
}

/// Semantic versioning for schemas.
#[derive(
    Archive,
    RkyvSerialize,
    RkyvDeserialize,
    SerdeSerialize,
    SerdeDeserialize,
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
)]
#[archive(check_bytes)]
pub struct SchemaVersion {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

impl SchemaVersion {
    pub fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Check if this version is compatible with another (same major)
    pub fn compatible_with(&self, other: &SchemaVersion) -> bool {
        self.major == other.major
    }
}

/// Field definition in a schema.
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
pub struct FieldDef {
    pub name: String,
    pub ty: String,              // Type as string (e.g., "u64", "String")
    pub attributes: Vec<String>, // e.g., "primary_key", "indexed"
    pub nullable: bool,
    pub default_value: Option<String>,
}

/// Metadata for schema documentation and constraints.
#[derive(
    Archive,
    RkyvSerialize,
    RkyvDeserialize,
    SerdeSerialize,
    SerdeDeserialize,
    Debug,
    Clone,
    PartialEq,
    Default,
)]
#[archive(check_bytes)]
pub struct SchemaMetadata {
    pub description: Option<String>,
    pub author: Option<String>,
    pub created_at: Option<u64>, // Unix timestamp
    pub updated_at: Option<u64>,
    pub constraints: Vec<SchemaConstraint>,
}

/// Schema constraints for validation.
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
pub enum SchemaConstraint {
    Unique {
        fields: Vec<String>,
    },
    Index {
        fields: Vec<String>,
        name: String,
    },
    Check {
        expression: String,
    },
    ForeignKey {
        field: String,
        references: String,
        on_delete: ReferentialAction,
    },
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
pub enum ReferentialAction {
    Cascade,
    SetNull,
    Restrict,
    NoAction,
}

/// Registry operations for schema management.
#[derive(
    Archive, RkyvSerialize, RkyvDeserialize, SerdeSerialize, SerdeDeserialize, Debug, Clone,
)]
#[archive(check_bytes)]
pub enum SchemaRegistryRequest {
    /// Register a new schema (declaration)
    Register { entry: SchemaEntry },

    /// Retrieve a schema by name (consumption)
    Get {
        name: String,
        version: Option<SchemaVersion>,
    },

    /// List all available schemas
    List { prefix: Option<String> },

    /// Check if schema exists and get compatibility info
    Check {
        name: String,
        version: SchemaVersion,
    },

    /// Evolve an existing schema (migration)
    Evolve {
        name: String,
        from_version: SchemaVersion,
        to_entry: SchemaEntry,
    },
}

#[derive(
    Archive, RkyvSerialize, RkyvDeserialize, SerdeSerialize, SerdeDeserialize, Debug, Clone,
)]
#[archive(check_bytes)]
pub enum SchemaRegistryResponse {
    /// Schema registered successfully
    Registered { version: SchemaVersion },

    /// Schema retrieved
    Found { entry: SchemaEntry },

    /// Schema list
    List { entries: Vec<SchemaEntry> },

    /// Compatibility check result
    Compatible {
        current: SchemaVersion,
        compatible: bool,
        migration_required: bool,
    },

    /// Schema evolved
    Evolved { new_version: SchemaVersion },

    /// Error response
    Error {
        code: SchemaErrorCode,
        message: String,
    },
}

#[derive(
    Archive,
    RkyvSerialize,
    RkyvDeserialize,
    SerdeSerialize,
    SerdeDeserialize,
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
)]
#[archive(check_bytes)]
pub enum SchemaErrorCode {
    NotFound,
    VersionMismatch,
    IncompatibleChange,
    InvalidField,
    AlreadyExists,
    InternalError,
}
