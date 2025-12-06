// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::{Context, Result};
use clap::Parser;
use convert_case::{Case, Casing};
use quote::ToTokens;
use std::fs;
use std::path::PathBuf;
use syn::{parse_file, Item, Type};

#[derive(Parser)]
struct Cli {
    #[arg(short, long)]
    cell: String,

    #[arg(short, long)]
    lang: String,

    #[arg(short, long)]
    out: PathBuf,
}

fn main() -> Result<()> {
    let args = Cli::parse();

    let home = dirs::home_dir().context("No home dir")?;
    let schema_path = home.join(".cell/schema").join(format!("{}.rs", args.cell));
    let lock_path = home
        .join(".cell/schema")
        .join(format!("{}.lock", args.cell));

    if !schema_path.exists() {
        anyhow::bail!(
            "Schema '{}' not found. Compile the Rust cell first.",
            args.cell
        );
    }

    // FIX: Use cell_build to flatten module structure
    let file = cell_build::load_and_flatten_source(&schema_path)?;

    // Scan for struct/enum in flattened file
    // Note: Items might be nested in mods now. 
    // Simplified scan for top-level or first match
    fn find_item<'a>(items: &'a [Item], name: &str) -> Option<&'a Item> {
        for item in items {
            match item {
                Item::Struct(s) if s.ident == name => return Some(item),
                Item::Enum(e) if e.ident == name => return Some(item),
                Item::Mod(m) => {
                    if let Some((_, content)) = &m.content {
                        if let Some(found) = find_item(content, name) {
                            return Some(found);
                        }
                    }
                }
                _ => {}
            }
        }
        None
    }

    let item = find_item(&file.items, &args.cell)
        .context("Schema definition not found in file")?;

    let ast_string = item.to_token_stream().to_string();
    let mut hasher = blake3::Hasher::new();
    hasher.update(ast_string.as_bytes());
    let hash_bytes = hasher.finalize();
    let hash_hex = hash_bytes.to_hex().to_string();
    let fp_u64 = u64::from_le_bytes(hash_bytes.as_bytes()[0..8].try_into()?);

    if lock_path.exists() {
        let expected = fs::read_to_string(lock_path)?.trim().to_string();
        if expected != hash_hex {
            anyhow::bail!("Schema Mismatch.\nLockfile: {}\nComputed: {}\nRun 'cell clean' or rebuild Rust cell.", expected, hash_hex);
        }
    }

    let output = match args.lang.as_str() {
        "go" => generate_go(item, fp_u64, &args.cell)?,
        "py" => generate_py(item, fp_u64, &args.cell)?,
        _ => anyhow::bail!("Unsupported language: {}", args.lang),
    };

    fs::write(&args.out, output)?;
    println!("Generated binding for {} ({})", args.cell, args.lang);
    Ok(())
}

fn generate_go(item: &Item, fp: u64, name: &str) -> Result<String> {
    let mut code = String::new();
    code.push_str("package main\n\n");
    code.push_str("import (\n\t\"encoding/binary\"\n\t\"bytes\"\n)\n\n");

    code.push_str(&format!("// Schema Fingerprint: 0x{:x}\n", fp));
    code.push_str(&format!(
        "const {}_Fingerprint uint64 = 0x{:x}\n\n",
        name, fp
    ));

    match item {
        Item::Struct(s) => {
            code.push_str(&format!("type {} struct {{\n", s.ident));
            for field in &s.fields {
                let fname = field
                    .ident
                    .as_ref()
                    .unwrap()
                    .to_string()
                    .to_case(Case::Pascal);
                let ftype = map_rust_type_to_go(&field.ty);
                code.push_str(&format!("\t{} {}\n", fname, ftype));
            }
            code.push_str("}\n\n");

            code.push_str(&format!("func (m *{}) Serialize() []byte {{\n", s.ident));
            code.push_str("\tbuf := new(bytes.Buffer)\n");

            for field in &s.fields {
                let fname = field
                    .ident
                    .as_ref()
                    .unwrap()
                    .to_string()
                    .to_case(Case::Pascal);
                match map_rust_type_to_go(&field.ty) {
                    "uint64" => code.push_str(&format!(
                        "\tbinary.Write(buf, binary.LittleEndian, m.{})\n",
                        fname
                    )),
                    "uint32" => code.push_str(&format!(
                        "\tbinary.Write(buf, binary.LittleEndian, m.{})\n",
                        fname
                    )),
                    "uint8" => code.push_str(&format!(
                        "\tbinary.Write(buf, binary.LittleEndian, m.{})\n",
                        fname
                    )),
                    _ => {}
                }
            }
            code.push_str("\treturn buf.Bytes()\n");
            code.push_str("}\n\n");

            code.push_str(&format!(
                "func Deserialize{}(data []byte) *{} {{\n",
                name, name
            ));
            code.push_str(&format!("\tres := &{}{{}}\n", name));
            code.push_str("\tbuf := bytes.NewReader(data)\n");
            for field in &s.fields {
                let fname = field
                    .ident
                    .as_ref()
                    .unwrap()
                    .to_string()
                    .to_case(Case::Pascal);
                match map_rust_type_to_go(&field.ty) {
                    "uint64" => code.push_str(&format!(
                        "\tbinary.Read(buf, binary.LittleEndian, &res.{})\n",
                        fname
                    )),
                    "uint32" => code.push_str(&format!(
                        "\tbinary.Read(buf, binary.LittleEndian, &res.{})\n",
                        fname
                    )),
                    "uint8" => code.push_str(&format!(
                        "\tbinary.Read(buf, binary.LittleEndian, &res.{})\n",
                        fname
                    )),
                    _ => {}
                }
            }
            code.push_str("\treturn res\n");
            code.push_str("}\n");
        }
        Item::Enum(_) => {
            code.push_str("// Enum support coming in v0.3.1\n");
        }
        _ => {}
    }

    Ok(code)
}

fn map_rust_type_to_go(ty: &Type) -> String {
    if let Type::Path(p) = ty {
        if let Some(seg) = p.path.segments.last() {
            let ident_str = seg.ident.to_string();
            return match ident_str.as_str() {
                "u64" => "uint64".to_string(),
                "u32" => "uint32".to_string(),
                "u8" => "uint8".to_string(),
                "i64" => "int64".to_string(),
                "String" => "string".to_string(),
                "bool" => "bool".to_string(),
                "Vec" => "[]byte".to_string(), // Simplified assumption for generic Vec
                other => other.to_string(), // FIX: Return actual type name for structs instead of casting to []byte
            };
        }
    }
    "[]byte".to_string()
}

fn generate_py(item: &Item, fp: u64, name: &str) -> Result<String> {
    Ok(format!(
        "import struct\n\n\
        SCHEMA_FINGERPRINT = 0x{:x}\n\n\
        class {}:\n\
            pass\n\
        # Full Python struct packing logic would go here\n",
        fp, name
    ))
}