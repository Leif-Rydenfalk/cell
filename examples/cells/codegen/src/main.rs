// cells/codegen/src/main.rs
// SPDX-License-Identifier: MIT
// Polyglot code generator for Python, Go, TypeScript, etc.

use cell_sdk::*;
use anyhow::Result;

// === PROTOCOL ===

#[protein]
pub struct GenerateRequest {
    pub cell_name: String,
    pub target_language: Language,
    pub options: GenerationOptions,
}

#[protein]
pub enum Language {
    Python,
    Go,
    TypeScript,
    Rust,
    C,
    Java,
}

#[protein]
pub struct GenerationOptions {
    pub package_name: Option<String>,
    pub async_client: bool,
    pub include_types: bool,
}

#[protein]
pub struct GenerateResponse {
    pub success: bool,
    pub code: String,
    pub auxiliary_files: Vec<AuxFile>,
}

#[protein]
pub struct AuxFile {
    pub filename: String,
    pub content: String,
}

#[protein]
pub struct SchemaInfo {
    pub types: Vec<TypeDef>,
    pub methods: Vec<MethodDef>,
}

#[protein]
pub struct TypeDef {
    pub name: String,
    pub kind: String, // "struct", "enum"
    pub fields: Vec<FieldDef>,
}

#[protein]
pub struct FieldDef {
    pub name: String,
    pub type_name: String,
}

#[protein]
pub struct MethodDef {
    pub name: String,
    pub inputs: Vec<FieldDef>,
    pub output: String,
}

// === CODE GENERATORS ===

struct PythonGenerator;
struct GoGenerator;
struct TypeScriptGenerator;

impl PythonGenerator {
    fn generate(schema: &SchemaInfo, cell_name: &str, _opts: &GenerationOptions) -> Result<String> {
        let mut code = String::new();
        code.push_str(&format!("# Client for {}\n", cell_name));
        for ty in &schema.types {
            code.push_str(&format!("class {}:\n    pass\n", ty.name));
        }
        Ok(code)
    }
}

impl GoGenerator {
    fn generate(schema: &SchemaInfo, cell_name: &str, _opts: &GenerationOptions) -> Result<String> {
        let mut code = String::new();
        code.push_str(&format!("// Client for {}\npackage main\n", cell_name));
        for ty in &schema.types {
             code.push_str(&format!("type {} struct {{}}\n", ty.name));
        }
        Ok(code)
    }
}

impl TypeScriptGenerator {
    fn generate(schema: &SchemaInfo, cell_name: &str, _opts: &GenerationOptions) -> Result<String> {
        let mut code = String::new();
        code.push_str(&format!("// Client for {}\n", cell_name));
        for ty in &schema.types {
             code.push_str(&format!("export interface {} {{}}\n", ty.name));
        }
        Ok(code)
    }
}

// === CODEGEN SERVICE ===

pub struct CodegenService;

#[handler]
impl CodegenService {
    pub async fn generate(&self, req: GenerateRequest) -> Result<GenerateResponse> {
        let schema = SchemaInfo { types: vec![], methods: vec![] };

        let code = match req.target_language {
            Language::Python => PythonGenerator::generate(&schema, &req.cell_name, &req.options)?,
            Language::Go => GoGenerator::generate(&schema, &req.cell_name, &req.options)?,
            Language::TypeScript => TypeScriptGenerator::generate(&schema, &req.cell_name, &req.options)?,
            _ => anyhow::bail!("Language not yet supported"),
        };

        Ok(GenerateResponse {
            success: true,
            code,
            auxiliary_files: vec![],
        })
    }

    pub async fn list_languages(&self) -> Result<Vec<String>> {
        Ok(vec!["python".to_string(), "go".to_string(), "typescript".to_string()])
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let codegen = CodegenService;
    println!("[Codegen] Polyglot generator active");
    codegen.serve("codegen").await
}