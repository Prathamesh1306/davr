use chrono::Utc;
use davr_storage::Database;
use davr_types::{Confidence, DavrError, ProjectId, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use tracing::info;
use tree_sitter::{Language, Node, Parser};
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
    pub parse_incomplete: bool,
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

    /// Parses a single source file into structured symbols and raw imports using Tree-sitter
    pub fn parse_file(&self, file_path: &Path, content: &str) -> Option<ParsedFile> {
        let ext = file_path.extension()?.to_str()?;
        let language = match ext {
            "rs" => "rust",
            "ts" | "tsx" | "js" | "jsx" => "typescript",
            "py" => "python",
            "go" => "go",
            _ => return None,
        };

        let (symbols, raw_imports, parse_incomplete) = parse_with_tree_sitter(language, content);

        let content_hash = blake3::hash(content.as_bytes()).to_hex().to_string();

        Some(ParsedFile {
            path: file_path.to_string_lossy().to_string(),
            language: language.into(),
            content_hash,
            parse_incomplete,
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
// Tree-sitter Language Parsers (Rust, TypeScript, Python, Go)
// =====================================================================

pub fn parse_with_tree_sitter(
    language_name: &str,
    content: &str,
) -> (Vec<SourceSymbol>, Vec<String>, bool) {
    let lang: Language = match language_name {
        "rust" => tree_sitter_rust::language(),
        "typescript" | "javascript" => tree_sitter_typescript::language_typescript(),
        "python" => tree_sitter_python::language(),
        "go" => tree_sitter_go::language(),
        _ => return (Vec::new(), Vec::new(), false),
    };

    let mut parser = Parser::new();
    if parser.set_language(&lang).is_err() {
        return (Vec::new(), Vec::new(), false);
    }

    let tree = match parser.parse(content, None) {
        Some(t) => t,
        None => return (Vec::new(), Vec::new(), true),
    };

    let root = tree.root_node();
    let has_error = root.has_error();

    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let source = content.as_bytes();

    match language_name {
        "rust" => extract_rust_symbols(root, source, &mut symbols, &mut imports),
        "typescript" | "javascript" => extract_ts_symbols(root, source, &mut symbols, &mut imports),
        "python" => extract_python_symbols(root, source, &mut symbols, &mut imports),
        "go" => extract_go_symbols(root, source, &mut symbols, &mut imports),
        _ => {}
    }

    (symbols, imports, has_error)
}

fn node_text<'a>(node: Node<'a>, source: &'a [u8]) -> &'a str {
    node.utf8_text(source).unwrap_or("")
}

fn make_symbol(node: Node, name: String, kind: SymbolKind) -> SourceSymbol {
    SourceSymbol {
        name,
        kind,
        start_line: node.start_position().row + 1,
        end_line: node.end_position().row + 1,
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
    }
}

// ---------------------------------------------------------------------
// Rust AST Visitor
// ---------------------------------------------------------------------
fn extract_rust_symbols(
    node: Node,
    source: &[u8],
    symbols: &mut Vec<SourceSymbol>,
    imports: &mut Vec<String>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_item" => {
                if let Some(name_node) = child.child_by_field_name("name") {
                    symbols.push(make_symbol(
                        child,
                        node_text(name_node, source).to_string(),
                        SymbolKind::Function,
                    ));
                }
            }
            "struct_item" => {
                if let Some(name_node) = child.child_by_field_name("name") {
                    symbols.push(make_symbol(
                        child,
                        node_text(name_node, source).to_string(),
                        SymbolKind::Struct,
                    ));
                }
            }
            "enum_item" => {
                if let Some(name_node) = child.child_by_field_name("name") {
                    symbols.push(make_symbol(
                        child,
                        node_text(name_node, source).to_string(),
                        SymbolKind::Enum,
                    ));
                }
            }
            "trait_item" => {
                if let Some(name_node) = child.child_by_field_name("name") {
                    symbols.push(make_symbol(
                        child,
                        node_text(name_node, source).to_string(),
                        SymbolKind::Interface,
                    ));
                }
            }
            "impl_item" => {
                // Functions inside impl items are methods
                let mut impl_cursor = child.walk();
                for impl_child in child.children(&mut impl_cursor) {
                    if impl_child.kind() == "declaration_list" {
                        let mut body_cursor = impl_child.walk();
                        for item in impl_child.children(&mut body_cursor) {
                            if item.kind() == "function_item" {
                                if let Some(fn_name) = item.child_by_field_name("name") {
                                    symbols.push(make_symbol(
                                        item,
                                        node_text(fn_name, source).to_string(),
                                        SymbolKind::Method,
                                    ));
                                }
                            }
                        }
                    }
                }
            }
            "const_item" | "static_item" => {
                if let Some(name_node) = child.child_by_field_name("name") {
                    symbols.push(make_symbol(
                        child,
                        node_text(name_node, source).to_string(),
                        SymbolKind::Const,
                    ));
                }
            }
            "use_declaration" => {
                if let Some(arg) = child.child_by_field_name("argument") {
                    imports.push(node_text(arg, source).to_string());
                } else {
                    let text = node_text(child, source);
                    let cleaned = text.trim_start_matches("use ").trim_end_matches(';').trim();
                    if !cleaned.is_empty() {
                        imports.push(cleaned.to_string());
                    }
                }
            }
            "mod_item" => {
                // Recurse into inline modules
                extract_rust_symbols(child, source, symbols, imports);
            }
            _ => {
                if child.child_count() > 0 {
                    extract_rust_symbols(child, source, symbols, imports);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------
// TypeScript / JavaScript AST Visitor
// ---------------------------------------------------------------------
fn extract_ts_symbols(
    node: Node,
    source: &[u8],
    symbols: &mut Vec<SourceSymbol>,
    imports: &mut Vec<String>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let target_node = if child.kind() == "export_statement" {
            child.child_by_field_name("declaration").unwrap_or(child)
        } else {
            child
        };

        match target_node.kind() {
            "function_declaration" => {
                if let Some(name_node) = target_node.child_by_field_name("name") {
                    symbols.push(make_symbol(
                        target_node,
                        node_text(name_node, source).to_string(),
                        SymbolKind::Function,
                    ));
                }
            }
            "class_declaration" => {
                if let Some(name_node) = target_node.child_by_field_name("name") {
                    symbols.push(make_symbol(
                        target_node,
                        node_text(name_node, source).to_string(),
                        SymbolKind::Class,
                    ));
                }
                // Inspect class methods
                if let Some(body) = target_node.child_by_field_name("body") {
                    let mut body_cursor = body.walk();
                    for item in body.children(&mut body_cursor) {
                        if item.kind() == "method_definition" {
                            if let Some(m_name) = item.child_by_field_name("name") {
                                symbols.push(make_symbol(
                                    item,
                                    node_text(m_name, source).to_string(),
                                    SymbolKind::Method,
                                ));
                            }
                        }
                    }
                }
            }
            "interface_declaration" => {
                if let Some(name_node) = target_node.child_by_field_name("name") {
                    symbols.push(make_symbol(
                        target_node,
                        node_text(name_node, source).to_string(),
                        SymbolKind::Interface,
                    ));
                }
            }
            "enum_declaration" => {
                if let Some(name_node) = target_node.child_by_field_name("name") {
                    symbols.push(make_symbol(
                        target_node,
                        node_text(name_node, source).to_string(),
                        SymbolKind::Enum,
                    ));
                }
            }
            "import_statement" => {
                if let Some(source_node) = child.child_by_field_name("source") {
                    let raw = node_text(source_node, source);
                    let cleaned = raw.trim_matches(|c| c == '\'' || c == '"');
                    imports.push(cleaned.to_string());
                }
            }
            _ => {
                if child.child_count() > 0 && child.kind() != "function_declaration" {
                    extract_ts_symbols(child, source, symbols, imports);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------
// Python AST Visitor
// ---------------------------------------------------------------------
fn extract_python_symbols(
    node: Node,
    source: &[u8],
    symbols: &mut Vec<SourceSymbol>,
    imports: &mut Vec<String>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_definition" => {
                if let Some(name_node) = child.child_by_field_name("name") {
                    symbols.push(make_symbol(
                        child,
                        node_text(name_node, source).to_string(),
                        SymbolKind::Function,
                    ));
                }
            }
            "class_definition" => {
                if let Some(name_node) = child.child_by_field_name("name") {
                    symbols.push(make_symbol(
                        child,
                        node_text(name_node, source).to_string(),
                        SymbolKind::Class,
                    ));
                }
                // Inspect methods inside class
                if let Some(body) = child.child_by_field_name("body") {
                    let mut body_cursor = body.walk();
                    for item in body.children(&mut body_cursor) {
                        if item.kind() == "function_definition" {
                            if let Some(fn_name) = item.child_by_field_name("name") {
                                symbols.push(make_symbol(
                                    item,
                                    node_text(fn_name, source).to_string(),
                                    SymbolKind::Method,
                                ));
                            }
                        }
                    }
                }
            }
            "import_statement" => {
                let text = node_text(child, source);
                let cleaned = text.trim_start_matches("import ").trim();
                imports.push(cleaned.to_string());
            }
            "import_from_statement" => {
                if let Some(mod_node) = child.child_by_field_name("module_name") {
                    imports.push(node_text(mod_node, source).to_string());
                } else {
                    let text = node_text(child, source);
                    imports.push(text.to_string());
                }
            }
            _ => {
                if child.child_count() > 0 && child.kind() != "function_definition" {
                    extract_python_symbols(child, source, symbols, imports);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------
// Go AST Visitor
// ---------------------------------------------------------------------
fn extract_go_symbols(
    node: Node,
    source: &[u8],
    symbols: &mut Vec<SourceSymbol>,
    imports: &mut Vec<String>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_declaration" => {
                if let Some(name_node) = child.child_by_field_name("name") {
                    symbols.push(make_symbol(
                        child,
                        node_text(name_node, source).to_string(),
                        SymbolKind::Function,
                    ));
                }
            }
            "method_declaration" => {
                if let Some(name_node) = child.child_by_field_name("name") {
                    symbols.push(make_symbol(
                        child,
                        node_text(name_node, source).to_string(),
                        SymbolKind::Method,
                    ));
                }
            }
            "type_declaration" => {
                let mut type_cursor = child.walk();
                for type_child in child.children(&mut type_cursor) {
                    if type_child.kind() == "type_spec" {
                        if let Some(name_node) = type_child.child_by_field_name("name") {
                            let type_node = type_child.child_by_field_name("type");
                            let kind = match type_node.map(|n| n.kind()) {
                                Some("interface_type") => SymbolKind::Interface,
                                _ => SymbolKind::Struct,
                            };
                            symbols.push(make_symbol(
                                type_child,
                                node_text(name_node, source).to_string(),
                                kind,
                            ));
                        }
                    }
                }
            }
            "import_declaration" => {
                let mut imp_cursor = child.walk();
                for imp_child in child.children(&mut imp_cursor) {
                    if imp_child.kind() == "import_spec" {
                        if let Some(path_node) = imp_child.child_by_field_name("path") {
                            let raw = node_text(path_node, source);
                            imports.push(raw.trim_matches('"').to_string());
                        }
                    }
                }
            }
            _ => {
                if child.child_count() > 0 {
                    extract_go_symbols(child, source, symbols, imports);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------
// Path Resolution and File Ignore Filtering
// ---------------------------------------------------------------------

fn is_ignored_ast_path(root: &Path, p: &Path) -> bool {
    let rel = match p.strip_prefix(root) {
        Ok(r) => r.to_string_lossy(),
        Err(_) => return false,
    };

    rel.starts_with(".git")
        || rel.starts_with(".davr")
        || rel.starts_with("target")
        || rel.starts_with("node_modules")
        || rel.starts_with("dist")
        || rel.starts_with("build")
        || rel.starts_with(".venv")
        || rel.starts_with("__pycache__")
        || rel.starts_with(".next")
}

fn resolve_import_path(
    from_file: &str,
    import_target: &str,
    file_map: &HashMap<String, ParsedFile>,
) -> Option<String> {
    if import_target.starts_with("./") || import_target.starts_with("../") {
        let from_dir = Path::new(from_file).parent()?;
        let resolved = from_dir.join(import_target);
        let normalized = normalize_path(&resolved);

        let candidates = [
            format!("{}.ts", normalized),
            format!("{}.tsx", normalized),
            format!("{}.js", normalized),
            format!("{}.jsx", normalized),
            format!("{}/index.ts", normalized),
            format!("{}/index.js", normalized),
            format!("{}.py", normalized),
            format!("{}.rs", normalized),
        ];

        for c in candidates {
            if file_map.contains_key(&c) {
                return Some(c);
            }
        }
    }

    None
}

fn normalize_path(path: &Path) -> String {
    let mut parts = Vec::new();
    for comp in path.components() {
        match comp {
            std::path::Component::Normal(c) => parts.push(c.to_string_lossy().to_string()),
            std::path::Component::ParentDir => {
                parts.pop();
            }
            _ => {}
        }
    }
    parts.join("/")
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_rust_symbols() {
        let code = r#"
use crate::storage::Database;

pub fn calculate_sum(a: i32, b: i32) -> i32 {
    a + b
}

pub struct UserAccount {
    pub id: u64,
}

pub enum UserRole {
    Admin,
    Member,
}
"#;
        let (symbols, imports, incomplete) = parse_with_tree_sitter("rust", code);
        assert!(!incomplete, "Valid code should not be incomplete");
        assert_eq!(symbols.len(), 3);
        assert_eq!(symbols[0].name, "calculate_sum");
        assert_eq!(symbols[0].kind, SymbolKind::Function);
        assert_eq!(symbols[1].name, "UserAccount");
        assert_eq!(symbols[1].kind, SymbolKind::Struct);
        assert_eq!(symbols[2].name, "UserRole");
        assert_eq!(symbols[2].kind, SymbolKind::Enum);
        assert!(!imports.is_empty());
    }

    #[test]
    fn test_parse_typescript_symbols() {
        let code = r#"
import { authenticate } from './auth';
export function loginUser(req: any) {}
export class AuthService {}
export interface TokenPayload {}
"#;
        let (symbols, imports, incomplete) = parse_with_tree_sitter("typescript", code);
        assert!(!incomplete);
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
        let (symbols, imports, incomplete) = parse_with_tree_sitter("python", code);
        assert!(!incomplete);
        assert_eq!(symbols.len(), 2);
        assert_eq!(symbols[0].name, "create_app");
        assert_eq!(symbols[1].name, "DatabaseConnection");
        assert_eq!(imports.len(), 2);
    }

    #[test]
    fn test_error_tolerant_parsing_partial_syntax() {
        // Rust code with broken/incomplete syntax inside a function during agent edit
        let code = r#"
pub fn valid_first_func() -> i32 {
    42
}

pub fn broken_syntax_in_progress() {
    let x = ; // syntax error: missing expression before semicolon
}

pub struct ValidTrailingStruct {
    pub value: String,
}
"#;
        let (symbols, _imports, incomplete) = parse_with_tree_sitter("rust", code);
        // Tree-sitter detects incomplete/erroneous syntax
        assert!(incomplete, "Must flag syntax error as incomplete");
        // Tree-sitter still error-tolerantly recovers the surrounding symbols!
        assert!(
            symbols.iter().any(|s| s.name == "valid_first_func"),
            "Should recover valid_first_func"
        );
        assert!(
            symbols
                .iter()
                .any(|s| s.name == "broken_syntax_in_progress"),
            "Should recover broken_syntax_in_progress function header"
        );
        assert!(
            symbols.iter().any(|s| s.name == "ValidTrailingStruct"),
            "Should recover ValidTrailingStruct"
        );
    }
}
