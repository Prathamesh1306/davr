use chrono::Utc;
use davr_storage::Database;
use davr_types::{Confidence, DavrError, ProjectId, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use tracing::info;
use walkdir::WalkDir;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Interface,
    Struct,
    Enum,
    Const,
    Import,
}

impl SymbolKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SymbolKind::Function => "function",
            SymbolKind::Method => "method",
            SymbolKind::Class => "class",
            SymbolKind::Interface => "interface",
            SymbolKind::Struct => "struct",
            SymbolKind::Enum => "enum",
            SymbolKind::Const => "const",
            SymbolKind::Import => "import",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceSymbol {
    pub name: String,
    pub kind: SymbolKind,
    pub start_line: usize,
    pub end_line: usize,
    pub start_byte: usize,
    pub end_byte: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawDependencyEdge {
    pub from_file: String,
    pub to_file: Option<String>,
    pub from_symbol: Option<String>,
    pub to_symbol: Option<String>,
    pub edge_kind: String,
    pub confidence: Confidence,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedFile {
    pub path: String,
    pub language: String,
    pub content_hash: String,
    pub symbols: Vec<SourceSymbol>,
    pub raw_imports: Vec<String>,
}

pub struct AstEngine;

impl Default for AstEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl AstEngine {
    pub fn new() -> Self {
        Self
    }

    /// Parses a single source file into structured symbols and raw imports
    pub fn parse_file(&self, file_path: &Path, content: &str) -> Option<ParsedFile> {
        let ext = file_path.extension()?.to_str()?;
        let language = match ext {
            "rs" => "rust",
            "ts" | "tsx" | "js" | "jsx" => "typescript",
            "py" => "python",
            "go" => "go",
            _ => return None,
        };

        let (symbols, raw_imports) = match language {
            "rust" => parse_rust(content),
            "typescript" => parse_typescript(content),
            "python" => parse_python(content),
            "go" => parse_go(content),
            _ => (Vec::new(), Vec::new()),
        };

        let content_hash = blake3::hash(content.as_bytes()).to_hex().to_string();

        Some(ParsedFile {
            path: file_path.to_string_lossy().to_string(),
            language: language.into(),
            content_hash,
            symbols,
            raw_imports,
        })
    }

    /// Scans, parses, and persists all source files, symbols, and dependency edges in SQLite
    pub fn index_project(
        &self,
        db: &Database,
        project_id: &ProjectId,
        project_root: &Path,
    ) -> Result<usize> {
        let mut parsed_files = Vec::new();
        let mut file_map = HashMap::new();

        for entry in WalkDir::new(project_root)
            .into_iter()
            .filter_entry(|e| !is_ignored_ast_path(project_root, e.path()))
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if path.is_file() {
                if let Ok(content) = fs::read_to_string(path) {
                    if let Some(mut parsed) = self.parse_file(path, &content) {
                        let rel_path = path
                            .strip_prefix(project_root)
                            .unwrap_or(path)
                            .to_string_lossy()
                            .to_string();
                        parsed.path = rel_path.clone();
                        file_map.insert(rel_path, parsed.clone());
                        parsed_files.push(parsed);
                    }
                }
            }
        }

        let total_files = parsed_files.len();
        let conn = db.inner();
        let now = Utc::now().timestamp_millis();

        // 1. Persist source_files and source_symbols
        for pf in &parsed_files {
            let source_file_id = format!("{}:{}", project_id.as_str(), pf.path);

            conn.execute(
                "INSERT INTO source_files (id, project_id, file_path, language, content_hash, last_parsed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(project_id, file_path) DO UPDATE SET
                   content_hash = excluded.content_hash,
                   last_parsed_at = excluded.last_parsed_at",
                rusqlite::params![
                    &source_file_id,
                    project_id.as_str(),
                    &pf.path,
                    &pf.language,
                    &pf.content_hash,
                    now,
                ],
            )
            .map_err(|e| DavrError::Database(e.to_string()))?;

            // Clear old symbols for this file
            let _ = conn.execute(
                "DELETE FROM source_symbols WHERE source_file_id = ?1",
                [&source_file_id],
            );

            for sym in &pf.symbols {
                let symbol_id = uuid::Uuid::new_v4().to_string();
                let _ = conn.execute(
                    "INSERT INTO source_symbols (id, source_file_id, symbol_name, symbol_kind, start_byte, end_byte, start_line, end_line)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    rusqlite::params![
                        &symbol_id,
                        &source_file_id,
                        &sym.name,
                        sym.kind.as_str(),
                        sym.start_byte as i64,
                        sym.end_byte as i64,
                        sym.start_line as i64,
                        sym.end_line as i64,
                    ],
                );
            }
        }

        // 2. Resolve cross-file dependency edges
        let mut edges = Vec::new();
        for pf in &parsed_files {
            let from_file_id = format!("{}:{}", project_id.as_str(), pf.path);

            for raw_import in &pf.raw_imports {
                // Try resolving import target file path
                if let Some(target_file) = resolve_import_path(&pf.path, raw_import, &file_map) {
                    let to_file_id = format!("{}:{}", project_id.as_str(), target_file);
                    edges.push((from_file_id.clone(), to_file_id, "import", "high"));
                }
            }
        }

        // Persist edges
        for (from_id, to_id, kind, conf) in edges {
            let _ = conn.execute(
                "INSERT INTO dependency_edges (from_file_id, to_file_id, edge_kind, confidence)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![&from_id, &to_id, kind, conf],
            );
        }

        info!(
            files = total_files,
            "Indexed project AST symbols and dependency graph"
        );
        Ok(total_files)
    }
}

// =====================================================================
// Language Parsers (Rust, TypeScript, Python, Go)
// =====================================================================

fn parse_rust(content: &str) -> (Vec<SourceSymbol>, Vec<String>) {
    let mut symbols = Vec::new();
    let mut imports = Vec::new();

    let fn_re =
        Regex::new(r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+([a-zA-Z0-9_]+)").unwrap();
    let struct_re =
        Regex::new(r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?struct\s+([a-zA-Z0-9_]+)").unwrap();
    let enum_re = Regex::new(r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?enum\s+([a-zA-Z0-9_]+)").unwrap();
    let trait_re = Regex::new(r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?trait\s+([a-zA-Z0-9_]+)").unwrap();
    let use_re = Regex::new(r"(?m)^\s*(?:pub\s+)?use\s+([a-zA-Z0-9_:]+)").unwrap();
    let mod_re = Regex::new(r"(?m)^\s*(?:pub\s+)?mod\s+([a-zA-Z0-9_]+);").unwrap();

    let lines: Vec<&str> = content.lines().collect();

    for (line_idx, line) in lines.iter().enumerate() {
        let line_num = line_idx + 1;

        if let Some(caps) = fn_re.captures(line) {
            if let Some(m) = caps.get(1) {
                symbols.push(SourceSymbol {
                    name: m.as_str().to_string(),
                    kind: SymbolKind::Function,
                    start_line: line_num,
                    end_line: line_num,
                    start_byte: 0,
                    end_byte: 0,
                });
            }
        } else if let Some(caps) = struct_re.captures(line) {
            if let Some(m) = caps.get(1) {
                symbols.push(SourceSymbol {
                    name: m.as_str().to_string(),
                    kind: SymbolKind::Struct,
                    start_line: line_num,
                    end_line: line_num,
                    start_byte: 0,
                    end_byte: 0,
                });
            }
        } else if let Some(caps) = enum_re.captures(line) {
            if let Some(m) = caps.get(1) {
                symbols.push(SourceSymbol {
                    name: m.as_str().to_string(),
                    kind: SymbolKind::Enum,
                    start_line: line_num,
                    end_line: line_num,
                    start_byte: 0,
                    end_byte: 0,
                });
            }
        } else if let Some(caps) = trait_re.captures(line) {
            if let Some(m) = caps.get(1) {
                symbols.push(SourceSymbol {
                    name: m.as_str().to_string(),
                    kind: SymbolKind::Interface,
                    start_line: line_num,
                    end_line: line_num,
                    start_byte: 0,
                    end_byte: 0,
                });
            }
        }

        if let Some(caps) = use_re.captures(line) {
            if let Some(m) = caps.get(1) {
                imports.push(m.as_str().to_string());
            }
        } else if let Some(caps) = mod_re.captures(line) {
            if let Some(m) = caps.get(1) {
                imports.push(m.as_str().to_string());
            }
        }
    }

    (symbols, imports)
}

fn parse_typescript(content: &str) -> (Vec<SourceSymbol>, Vec<String>) {
    let mut symbols = Vec::new();
    let mut imports = Vec::new();

    let fn_re =
        Regex::new(r"(?m)^\s*(?:export\s+)?(?:async\s+)?function\s+([a-zA-Z0-9_]+)").unwrap();
    let class_re = Regex::new(r"(?m)^\s*(?:export\s+)?class\s+([a-zA-Z0-9_]+)").unwrap();
    let iface_re = Regex::new(r"(?m)^\s*(?:export\s+)?interface\s+([a-zA-Z0-9_]+)").unwrap();
    let type_re = Regex::new(r"(?m)^\s*(?:export\s+)?type\s+([a-zA-Z0-9_]+)").unwrap();
    let const_re = Regex::new(r"(?m)^\s*(?:export\s+)?const\s+([a-zA-Z0-9_]+)\s*=").unwrap();
    let import_re = Regex::new(r#"(?m)^\s*import\s+.*?from\s+['"]([^'"]+)['"]"#).unwrap();

    let lines: Vec<&str> = content.lines().collect();

    for (line_idx, line) in lines.iter().enumerate() {
        let line_num = line_idx + 1;

        if let Some(caps) = fn_re.captures(line) {
            if let Some(m) = caps.get(1) {
                symbols.push(SourceSymbol {
                    name: m.as_str().to_string(),
                    kind: SymbolKind::Function,
                    start_line: line_num,
                    end_line: line_num,
                    start_byte: 0,
                    end_byte: 0,
                });
            }
        } else if let Some(caps) = class_re.captures(line) {
            if let Some(m) = caps.get(1) {
                symbols.push(SourceSymbol {
                    name: m.as_str().to_string(),
                    kind: SymbolKind::Class,
                    start_line: line_num,
                    end_line: line_num,
                    start_byte: 0,
                    end_byte: 0,
                });
            }
        } else if let Some(caps) = iface_re.captures(line) {
            if let Some(m) = caps.get(1) {
                symbols.push(SourceSymbol {
                    name: m.as_str().to_string(),
                    kind: SymbolKind::Interface,
                    start_line: line_num,
                    end_line: line_num,
                    start_byte: 0,
                    end_byte: 0,
                });
            }
        } else if let Some(caps) = type_re.captures(line) {
            if let Some(m) = caps.get(1) {
                symbols.push(SourceSymbol {
                    name: m.as_str().to_string(),
                    kind: SymbolKind::Struct,
                    start_line: line_num,
                    end_line: line_num,
                    start_byte: 0,
                    end_byte: 0,
                });
            }
        } else if let Some(caps) = const_re.captures(line) {
            if let Some(m) = caps.get(1) {
                symbols.push(SourceSymbol {
                    name: m.as_str().to_string(),
                    kind: SymbolKind::Const,
                    start_line: line_num,
                    end_line: line_num,
                    start_byte: 0,
                    end_byte: 0,
                });
            }
        }

        if let Some(caps) = import_re.captures(line) {
            if let Some(m) = caps.get(1) {
                imports.push(m.as_str().to_string());
            }
        }
    }

    (symbols, imports)
}

fn parse_python(content: &str) -> (Vec<SourceSymbol>, Vec<String>) {
    let mut symbols = Vec::new();
    let mut imports = Vec::new();

    let def_re = Regex::new(r"(?m)^\s*(?:async\s+)?def\s+([a-zA-Z0-9_]+)\s*\(").unwrap();
    let class_re = Regex::new(r"(?m)^\s*class\s+([a-zA-Z0-9_]+)").unwrap();
    let import_re = Regex::new(r"(?m)^\s*import\s+([a-zA-Z0-9_.]+)").unwrap();
    let from_re = Regex::new(r"(?m)^\s*from\s+([a-zA-Z0-9_.]+)\s+import").unwrap();

    let lines: Vec<&str> = content.lines().collect();

    for (line_idx, line) in lines.iter().enumerate() {
        let line_num = line_idx + 1;

        if let Some(caps) = def_re.captures(line) {
            if let Some(m) = caps.get(1) {
                symbols.push(SourceSymbol {
                    name: m.as_str().to_string(),
                    kind: SymbolKind::Function,
                    start_line: line_num,
                    end_line: line_num,
                    start_byte: 0,
                    end_byte: 0,
                });
            }
        } else if let Some(caps) = class_re.captures(line) {
            if let Some(m) = caps.get(1) {
                symbols.push(SourceSymbol {
                    name: m.as_str().to_string(),
                    kind: SymbolKind::Class,
                    start_line: line_num,
                    end_line: line_num,
                    start_byte: 0,
                    end_byte: 0,
                });
            }
        }

        if let Some(caps) = import_re.captures(line) {
            if let Some(m) = caps.get(1) {
                imports.push(m.as_str().to_string());
            }
        } else if let Some(caps) = from_re.captures(line) {
            if let Some(m) = caps.get(1) {
                imports.push(m.as_str().to_string());
            }
        }
    }

    (symbols, imports)
}

fn parse_go(content: &str) -> (Vec<SourceSymbol>, Vec<String>) {
    let mut symbols = Vec::new();
    let mut imports = Vec::new();

    let func_re = Regex::new(r"(?m)^\s*func\s+(?:\([^)]*\)\s+)?([a-zA-Z0-9_]+)\s*\(").unwrap();
    let type_re = Regex::new(r"(?m)^\s*type\s+([a-zA-Z0-9_]+)\s+(?:struct|interface)").unwrap();
    let import_re = Regex::new(r#"(?m)^\s*(?:import\s+)?["']([^"']+)["']"#).unwrap();

    let lines: Vec<&str> = content.lines().collect();

    for (line_idx, line) in lines.iter().enumerate() {
        let line_num = line_idx + 1;

        if let Some(caps) = func_re.captures(line) {
            if let Some(m) = caps.get(1) {
                symbols.push(SourceSymbol {
                    name: m.as_str().to_string(),
                    kind: SymbolKind::Function,
                    start_line: line_num,
                    end_line: line_num,
                    start_byte: 0,
                    end_byte: 0,
                });
            }
        } else if let Some(caps) = type_re.captures(line) {
            if let Some(m) = caps.get(1) {
                symbols.push(SourceSymbol {
                    name: m.as_str().to_string(),
                    kind: SymbolKind::Struct,
                    start_line: line_num,
                    end_line: line_num,
                    start_byte: 0,
                    end_byte: 0,
                });
            }
        }

        if let Some(caps) = import_re.captures(line) {
            if let Some(m) = caps.get(1) {
                let imp = m.as_str();
                if imp.contains('/') || !imp.starts_with("std") {
                    imports.push(imp.to_string());
                }
            }
        }
    }

    (symbols, imports)
}

fn resolve_import_path(
    from_file: &str,
    import_str: &str,
    file_map: &HashMap<String, ParsedFile>,
) -> Option<String> {
    let from_path = Path::new(from_file);
    let parent = from_path.parent().unwrap_or(Path::new(""));

    // Relative import (./ or ../)
    if import_str.starts_with('.') {
        let joined = parent.join(import_str);
        for ext in &["", ".ts", ".tsx", ".js", ".jsx", "/index.ts", "/index.js"] {
            let candidate = joined.to_string_lossy().to_string() + ext;
            if file_map.contains_key(&candidate) {
                return Some(candidate);
            }
        }
    }

    // Module / crate import
    for path in file_map.keys() {
        if path.contains(import_str) {
            return Some(path.clone());
        }
    }

    None
}

fn is_ignored_ast_path(project_root: &Path, path: &Path) -> bool {
    let rel = match path.strip_prefix(project_root) {
        Ok(r) => r,
        Err(_) => path,
    };
    let path_str = rel.to_string_lossy();

    path_str.starts_with(".git")
        || path_str.starts_with(".davr")
        || path_str.contains("node_modules")
        || path_str.contains("/target/")
        || path_str.starts_with("target")
        || path_str.contains("/.venv/")
        || path_str.starts_with(".venv")
        || path_str.contains("/__pycache__/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_parse_rust_symbols() {
        let code = r#"
pub fn calculate_sum(a: i32, b: i32) -> i32 { a + b }
pub struct UserAccount { id: u64 }
enum UserRole { Admin, Member }
use crate::storage::Database;
"#;
        let (symbols, imports) = parse_rust(code);
        assert_eq!(symbols.len(), 3);
        assert_eq!(symbols[0].name, "calculate_sum");
        assert_eq!(symbols[0].kind, SymbolKind::Function);
        assert_eq!(symbols[1].name, "UserAccount");
        assert_eq!(symbols[1].kind, SymbolKind::Struct);
        assert_eq!(symbols[2].name, "UserRole");
        assert_eq!(symbols[2].kind, SymbolKind::Enum);
        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0], "crate::storage::Database");
    }

    #[test]
    fn test_parse_typescript_symbols() {
        let code = r#"
import { authenticate } from './auth';
export function loginUser(req: any) {}
export class AuthService {}
export interface TokenPayload {}
"#;
        let (symbols, imports) = parse_typescript(code);
        assert_eq!(symbols.len(), 3);
        assert_eq!(symbols[0].name, "loginUser");
        assert_eq!(symbols[1].name, "AuthService");
        assert_eq!(symbols[2].name, "TokenPayload");
        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0], "./auth");
    }

    #[test]
    fn test_parse_python_symbols() {
        let code = r#"
from fastapi import FastAPI
import os

def create_app():
    pass

class DatabaseConnection:
    pass
"#;
        let (symbols, imports) = parse_python(code);
        assert_eq!(symbols.len(), 2);
        assert_eq!(symbols[0].name, "create_app");
        assert_eq!(symbols[1].name, "DatabaseConnection");
        assert_eq!(imports.len(), 2);
    }
}
