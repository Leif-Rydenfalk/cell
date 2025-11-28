use anyhow::Result;
use regex::Regex;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::sys_log;

pub struct CellMeta {
    pub name: String,
    pub path: PathBuf,
    pub provides: Vec<String>, // Names of receptors defined here
    pub consumes: Vec<String>, // Names of cells called from here
}

pub fn scan_cell_dependencies(cell_root: &Path) -> Result<CellMeta> {
    let src_dir = cell_root.join("src");

    let mut meta = CellMeta {
        name: String::new(),
        path: cell_root.to_path_buf(),
        provides: Vec::new(),
        consumes: Vec::new(),
    };

    if !src_dir.exists() {
        return Ok(meta);
    }

    let re_call = Regex::new(r"call_as!\s*\(\s*([a-zA-Z0-9_]+)")?;
    let re_receptor = Regex::new(r"signal_receptor!\s*\{\s*name:\s*([a-zA-Z0-9_]+)")?;

    // Pass a mutable closure (&mut |entry| ...)
    visit_dirs(&src_dir, &mut |entry| {
        let path = entry.path();
        if path.extension().map_or(false, |e| e == "rs") {
            let content = fs::read_to_string(&path)?;
            let clean = strip_comments(&content);

            for cap in re_receptor.captures_iter(&clean) {
                meta.provides.push(cap[1].to_string());
            }

            for cap in re_call.captures_iter(&clean) {
                meta.consumes.push(cap[1].to_string());
            }
        }
        Ok(())
    })?;

    meta.consumes.sort();
    meta.consumes.dedup();

    Ok(meta)
}

/// Recursively scans the project's `src` folder for `signal_receptor!` definitions
/// and generates the `.cell/data/{name}.json` files required for compilation.
pub fn run_genesis(root: &Path) -> Result<()> {
    let src_dir = root.join("src");

    // Unified directory structure
    let cell_dir = root.join(".cell");
    let schema_dir = cell_dir.join("data");

    // If src doesn't exist (e.g. workspace root), we skip silently
    if !src_dir.exists() {
        return Ok(());
    }

    std::fs::create_dir_all(&schema_dir)?;

    // 1. Compile Regex once
    // Matches: signal_receptor! { name: foo, input: Bar ... output: Baz ... }
    // (?s) enables "dot matches newline" to handle multi-line macro invocations.
    let re = Regex::new(
        r"(?s)signal_receptor!\s*\{\s*name:\s*([a-zA-Z0-9_]+)\s*,\s*input:\s*([a-zA-Z0-9_]+).*?output:\s*([a-zA-Z0-9_]+)",
    )?;

    // 2. Recursive Walk
    visit_dirs(&src_dir, &mut |entry| {
        process_file(entry.path(), &schema_dir, &re)
    })?;

    Ok(())
}

/// Helper to recursively walk directories
fn visit_dirs(dir: &Path, cb: &mut dyn FnMut(&fs::DirEntry) -> Result<()>) -> io::Result<()> {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                visit_dirs(&path, cb)?;
            } else {
                cb(&entry).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
            }
        }
    }
    Ok(())
}

/// Analyzes a single file
fn process_file(path: PathBuf, schema_dir: &Path, re: &Regex) -> Result<()> {
    // Only check Rust files
    if path.extension().map_or(false, |ext| ext == "rs") {
        let content = fs::read_to_string(&path)?;

        // Strip comments to avoid parsing commented-out macros
        let clean_content = strip_comments(&content);

        // Iterate over all matches in the file (a file might define multiple receptors)
        for cap in re.captures_iter(&clean_content) {
            let cell_name = &cap[1];
            let input_type = &cap[2];
            let output_type = &cap[3];

            let json = format!(
                r#"{{ "input": "{}", "output": "{}" }}"#,
                input_type, output_type
            );

            let dest = schema_dir.join(format!("{}.json", cell_name));
            fs::write(&dest, json)?;

            sys_log(
                "INFO",
                &format!("Genesis: Synthesized schema for '{}'", cell_name),
            );
        }
    }
    Ok(())
}

/// Rudimentary comment stripper to prevent false positives.
/// Removes // line comments and /* block comments */.
fn strip_comments(code: &str) -> String {
    let mut result = String::with_capacity(code.len());
    let mut chars = code.chars().peekable();
    let mut in_string = false;

    while let Some(c) = chars.next() {
        if in_string {
            result.push(c);
            if c == '"' {
                in_string = false;
            }
            // Handle escaped quotes inside string
            if c == '\\' {
                if let Some(next) = chars.next() {
                    result.push(next);
                }
            }
        } else {
            if c == '"' {
                in_string = true;
                result.push(c);
            } else if c == '/' {
                match chars.peek() {
                    Some('/') => {
                        // Line comment: Skip until newline
                        chars.next(); // consume 2nd /
                        while let Some(n) = chars.next() {
                            if n == '\n' {
                                result.push('\n');
                                break;
                            }
                        }
                    }
                    Some('*') => {
                        // Block comment: Skip until */
                        chars.next(); // consume *
                        while let Some(n) = chars.next() {
                            if n == '*' {
                                if let Some('/') = chars.peek() {
                                    chars.next(); // consume /
                                    break;
                                }
                            }
                        }
                    }
                    _ => result.push(c), // Just a division sign
                }
            } else {
                result.push(c);
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_strip_comments() {
        let code = r#"
            // This is a comment
            signal_receptor! { name: valid, input: A, output: B }
            /* Block comment 
               signal_receptor! { name: invalid, input: X, output: Y }
            */
            let s = "string with // comment chars";
        "#;

        let cleaned = strip_comments(code);
        assert!(cleaned.contains("name: valid"));
        assert!(!cleaned.contains("name: invalid"));
        assert!(cleaned.contains("string with // comment chars"));
    }

    #[test]
    fn test_genesis_discovery() -> Result<()> {
        let dir = tempdir()?;
        let src = dir.path().join("src");
        fs::create_dir(&src)?;

        // 1. Standard formatting
        fs::write(
            src.join("main.rs"),
            r#"
            signal_receptor! {
                name: standard,
                input: Request,
                output: Response
            }
        "#,
        )?;

        // 2. Minified/One-liner
        fs::write(
            src.join("compact.rs"),
            r#"signal_receptor!{name:compact,input:In,output:Out}"#,
        )?;

        // 3. Commented out (Should NOT be found)
        fs::write(
            src.join("ignored.rs"),
            r#"
            // signal_receptor! { name: ghost, input: A, output: B }
        "#,
        )?;

        // 4. Deeply nested file
        let deep = src.join("deep/nested");
        fs::create_dir_all(&deep)?;
        fs::write(
            deep.join("mod.rs"),
            r#"
            pub mod inner {
                signal_receptor! {
                    name: nested_cell,
                    input: DeepIn,
                    output: DeepOut
                }
            }
        "#,
        )?;

        // Run Genesis
        run_genesis(dir.path())?;

        let schema_dir = dir.path().join(".cell").join("data");

        // Assertions
        assert!(schema_dir.join("standard.json").exists());
        assert!(schema_dir.join("compact.json").exists());
        assert!(schema_dir.join("nested_cell.json").exists());
        assert!(!schema_dir.join("ghost.json").exists());

        // Verify Content
        let standard_json = fs::read_to_string(schema_dir.join("standard.json"))?;
        assert_eq!(
            standard_json,
            r#"{ "input": "Request", "output": "Response" }"#
        );

        Ok(())
    }
}
