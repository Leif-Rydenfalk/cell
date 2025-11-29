use anyhow::{Context, Result};
use clap::Parser;
use convert_case::{Case, Casing};
use quote::ToTokens;
use std::fs;
use std::path::PathBuf;
use syn::{parse_file, Data, DeriveInput, Fields, Item, Type};

#[derive(Parser)]
struct Cli {
    /// The cell class name (e.g., "DadMsg")
    #[arg(short, long)]
    cell: String,

    /// Target language (go, py)
    #[arg(short, long)]
    lang: String,

    /// Output file
    #[arg(short, long)]
    out: PathBuf,
}

fn main() -> Result<()> {
    let args = Cli::parse();

    // 1. Locate the Schema
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

    let content = fs::read_to_string(&schema_path)?;
    let syntax = parse_file(&content)?;

    // 2. Find the Struct/Enum matching the cell name
    let item = syntax
        .items
        .iter()
        .find(|i| match i {
            Item::Struct(s) => s.ident == args.cell,
            Item::Enum(e) => e.ident == args.cell,
            _ => false,
        })
        .context("Schema definition not found in file")?;

    // 3. Compute Fingerprint (Must match cell-macros logic)
    // We quote the item back to string to normalize formatting for hashing
    let ast_string = item.to_token_stream().to_string();
    let mut hasher = blake3::Hasher::new();
    hasher.update(ast_string.as_bytes());
    let hash_bytes = hasher.finalize();
    let hash_hex = hash_bytes.to_hex().to_string();
    let fp_u64 = u64::from_le_bytes(hash_bytes.as_bytes()[0..8].try_into()?);

    // 4. Verify against Lockfile (Bind-time Safety)
    if lock_path.exists() {
        let expected = fs::read_to_string(lock_path)?.trim().to_string();
        if expected != hash_hex {
            anyhow::bail!("Schema Mismatch.\nLockfile: {}\nComputed: {}\nRun 'cell clean' or rebuild Rust cell.", expected, hash_hex);
        }
    }

    // 5. Generate Code
    let output = match args.lang.as_str() {
        "go" => generate_go(item, fp_u64, &args.cell)?,
        "py" => generate_py(item, fp_u64, &args.cell)?,
        _ => anyhow::bail!("Unsupported language: {}", args.lang),
    };

    fs::write(&args.out, output)?;
    println!("Generated binding for {} ({})", args.cell, args.lang);
    Ok(())
}

// --- Go Generator ---

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

            // Generate Packer (Serializer)
            code.push_str(&format!("func (m *{}) Serialize() []byte {{\n", s.ident));
            code.push_str("\tbuf := new(bytes.Buffer)\n");

            // Simple Rkyv-like Packer for fixed layout
            // NOTE: This is a simplified packer for the POD examples.
            // Full Rkyv support would require relative pointer tracking.
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
                    _ => {} // Handle others
                }
            }
            code.push_str("\treturn buf.Bytes()\n");
            code.push_str("}\n\n");

            // Generate Unpacker (Deserializer)
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
            // For MarketMsg (Enum), we generate a simplified struct approach or interface.
            // For this demo, we handle the specific structs used in the example manually via `cell-bind`
            // logic expansion would go here.
            code.push_str("// Enum support coming in v0.3.1\n");
        }
        _ => {}
    }

    Ok(code)
}

fn map_rust_type_to_go(ty: &Type) -> &'static str {
    if let Type::Path(p) = ty {
        if let Some(seg) = p.path.segments.last() {
            return match seg.ident.to_string().as_str() {
                "u64" => "uint64",
                "u32" => "uint32",
                "u8" => "uint8",
                "i64" => "int64",
                "String" => "string",
                "bool" => "bool",
                _ => "[]byte", // Fallback
            };
        }
    }
    "[]byte"
}

// --- Python Generator ---

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
