// cells/codegen/src/main.rs
// SPDX-License-Identifier: MIT
// Polyglot code generator for Python, Go, TypeScript, etc.

use cell_sdk::*;
use anyhow::Result;
use std::collections::HashMap;

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
    fn generate(schema: &SchemaInfo, cell_name: &str, opts: &GenerationOptions) -> Result<String> {
        let mut code = String::new();
        
        code.push_str(&format!(r#"# Auto-generated client for {}
import msgpack
import socket
import struct
from dataclasses import dataclass
from typing import List, Optional

"#, cell_name));

        // Generate types
        for ty in &schema.types {
            code.push_str(&format!("@dataclass\nclass {}:\n", ty.name));
            for field in &ty.fields {
                let py_type = Self::map_type(&field.type_name);
                code.push_str(&format!("    {}: {}\n", field.name, py_type));
            }
            code.push_str("\n");
        }

        // Generate client
        code.push_str(&format!(r#"
class {}Client:
    def __init__(self, socket_path=None):
        self.socket_path = socket_path or f"/tmp/cell/{}.sock"
        self.sock = None

    def connect(self):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(self.socket_path)

    def close(self):
        if self.sock:
            self.sock.close()

    def _send_request(self, method, data):
        payload = msgpack.packb({{'method': method, 'data': data}})
        length = struct.pack('<I', len(payload))
        self.sock.sendall(length + payload)
        
        resp_len = struct.unpack('<I', self.sock.recv(4))[0]
        resp_data = self.sock.recv(resp_len)
        return msgpack.unpackb(resp_data)

"#, cell_name, cell_name));

        // Generate methods
        for method in &schema.methods {
            let args: Vec<String> = method.inputs.iter()
                .map(|f| format!("{}: {}", f.name, Self::map_type(&f.type_name)))
                .collect();
            
            code.push_str(&format!(r#"    def {}(self, {}):
        request = {{{}}}
        return self._send_request('{}', request)

"#, 
                method.name,
                args.join(", "),
                method.inputs.iter()
                    .map(|f| format!("'{}': {}", f.name, f.name))
                    .collect::<Vec<_>>()
                    .join(", "),
                method.name
            ));
        }

        Ok(code)
    }

    fn map_type(rust_type: &str) -> &'static str {
        match rust_type {
            "String" => "str",
            "u64" | "u32" | "i64" | "i32" => "int",
            "f64" | "f32" => "float",
            "bool" => "bool",
            _ if rust_type.starts_with("Vec<") => "List",
            _ => "Any",
        }
    }
}

impl GoGenerator {
    fn generate(schema: &SchemaInfo, cell_name: &str, opts: &GenerationOptions) -> Result<String> {
        let mut code = String::new();
        
        code.push_str(&format!(r#"// Auto-generated client for {}
package main

import (
    "encoding/binary"
    "fmt"
    "net"
)

"#, cell_name));

        // Generate types
        for ty in &schema.types {
            code.push_str(&format!("type {} struct {{\n", ty.name));
            for field in &ty.fields {
                let go_type = Self::map_type(&field.type_name);
                code.push_str(&format!("    {} {}\n", 
                    Self::capitalize(&field.name), go_type));
            }
            code.push_str("}\n\n");
        }

        // Generate client
        code.push_str(&format!(r#"type {}Client struct {{
    conn net.Conn
}}

func New{}Client(socketPath string) (*{}Client, error) {{
    if socketPath == "" {{
        socketPath = "/tmp/cell/{}.sock"
    }}
    conn, err := net.Dial("unix", socketPath)
    if err != nil {{
        return nil, err
    }}
    return &{}Client{{conn: conn}}, nil
}}

func (c *{}Client) Close() error {{
    return c.conn.Close()
}}

"#, cell_name, cell_name, cell_name, cell_name, cell_name, cell_name));

        // Generate methods
        for method in &schema.methods {
            code.push_str(&format!(r#"func (c *{}Client) {}() error {{
    // TODO: Implement serialization
    return nil
}}

"#, cell_name, Self::capitalize(&method.name)));
        }

        Ok(code)
    }

    fn map_type(rust_type: &str) -> &'static str {
        match rust_type {
            "String" => "string",
            "u64" => "uint64",
            "u32" => "uint32",
            "i64" => "int64",
            "i32" => "int32",
            "f64" => "float64",
            "bool" => "bool",
            _ if rust_type.starts_with("Vec<") => "[]interface{}",
            _ => "interface{}",
        }
    }

    fn capitalize(s: &str) -> String {
        let mut chars = s.chars();
        match chars.next() {
            None => String::new(),
            Some(first) => first.to_uppercase().chain(chars).collect(),
        }
    }
}

impl TypeScriptGenerator {
    fn generate(schema: &SchemaInfo, cell_name: &str, opts: &GenerationOptions) -> Result<String> {
        let mut code = String::new();
        
        code.push_str(&format!(r#"// Auto-generated client for {}
import * as net from 'net';
import msgpack from 'msgpack-lite';

"#, cell_name));

        // Generate types
        for ty in &schema.types {
            code.push_str(&format!("export interface {} {{\n", ty.name));
            for field in &ty.fields {
                let ts_type = Self::map_type(&field.type_name);
                code.push_str(&format!("  {}: {};\n", field.name, ts_type));
            }
            code.push_str("}\n\n");
        }

        // Generate client
        code.push_str(&format!(r#"export class {}Client {{
  private socket: net.Socket | null = null;

  constructor(private socketPath: string = '/tmp/cell/{}.sock') {{}}

  async connect(): Promise<void> {{
    return new Promise((resolve, reject) => {{
      this.socket = net.createConnection(this.socketPath);
      this.socket.on('connect', () => resolve());
      this.socket.on('error', reject);
    }});
  }}

  close(): void {{
    this.socket?.end();
  }}

  private async sendRequest(method: string, data: any): Promise<any> {{
    const payload = msgpack.encode({{ method, data }});
    const length = Buffer.alloc(4);
    length.writeUInt32LE(payload.length);
    
    this.socket!.write(length);
    this.socket!.write(payload);
    
    return new Promise((resolve, reject) => {{
      this.socket!.once('data', (buf) => {{
        const respLen = buf.readUInt32LE(0);
        const respData = buf.slice(4, 4 + respLen);
        resolve(msgpack.decode(respData));
      }});
    }});
  }}

"#, cell_name, cell_name));

        // Generate methods
        for method in &schema.methods {
            let args: Vec<String> = method.inputs.iter()
                .map(|f| format!("{}: {}", f.name, Self::map_type(&f.type_name)))
                .collect();
            
            code.push_str(&format!(r#"  async {}({}): Promise<{}> {{
    const request = {{ {} }};
    return this.sendRequest('{}', request);
  }}

"#, 
                method.name,
                args.join(", "),
                Self::map_type(&method.output),
                method.inputs.iter()
                    .map(|f| &f.name)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", "),
                method.name
            ));
        }

        code.push_str("}\n");
        Ok(code)
    }

    fn map_type(rust_type: &str) -> &'static str {
        match rust_type {
            "String" => "string",
            "u64" | "u32" | "i64" | "i32" => "number",
            "f64" | "f32" => "number",
            "bool" => "boolean",
            "()" => "void",
            _ if rust_type.starts_with("Vec<") => "any[]",
            _ => "any",
        }
    }
}

// === CODEGEN SERVICE ===

pub struct CodegenService;

#[handler]
impl CodegenService {
    pub async fn generate(&self, req: GenerateRequest) -> Result<GenerateResponse> {
        // Get schema from nucleus
        let schema = SchemaInfo {
            types: vec![],
            methods: vec![],
        };

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
        Ok(vec![
            "python".to_string(),
            "go".to_string(),
            "typescript".to_string(),
            "rust".to_string(),
        ])
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let codegen = CodegenService;
    
    println!("[Codegen] Polyglot generator active");
    codegen.serve("codegen").await
}