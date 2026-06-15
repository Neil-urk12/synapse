pub mod cpp;
pub mod go;
pub mod java_kotlin;
pub mod javascript;
pub mod php;
pub mod python;
pub mod ruby;
pub mod rust;
pub mod swift;

use crate::types::ast::{EdgeData, FileAnalysis, NodeData, RawCall, RawImport};
use std::path::Path;
use tree_sitter::Node;

pub trait LanguageParser {
    fn parse(&self, content: &str, file_path: &str) -> FileAnalysis;
}

pub enum Language {
    Rust(rust::RustParser),
    JsTs(javascript::JsTsParser),
    Go(go::GoParser),
    Python(python::PythonParser),
    Cpp(cpp::CppParser),
    JavaKotlin(java_kotlin::JavaKotlinParser),
    Ruby(ruby::RubyParser),
    Php(php::PhpParser),
    Swift(swift::SwiftParser),
}

impl Language {
    fn parse(&self, content: &str, file_path: &str) -> FileAnalysis {
        match self {
            Language::Rust(p) => p.parse(content, file_path),
            Language::JsTs(p) => p.parse(content, file_path),
            Language::Go(p) => p.parse(content, file_path),
            Language::Python(p) => p.parse(content, file_path),
            Language::Cpp(p) => p.parse(content, file_path),
            Language::JavaKotlin(p) => p.parse(content, file_path),
            Language::Ruby(p) => p.parse(content, file_path),
            Language::Php(p) => p.parse(content, file_path),
            Language::Swift(p) => p.parse(content, file_path),
        }
    }
}

/// Look up the parser for a file by its extension. Single source of truth
/// for "is this file parseable by Synapse?". Replaces the per-caller
/// hardcoded extension lists that previously caused the index and watch
/// paths to disagree (e.g. Go/Python/C++ files indexed as File nodes with
/// no Symbols).
pub fn language_from_path(path: &Path) -> Option<Language> {
    path.extension()
        .and_then(|e| e.to_str())
        .and_then(language_from_ext)
}

fn language_from_ext(ext: &str) -> Option<Language> {
    match ext {
        "rs" => Some(Language::Rust(rust::RustParser)),
        "js" | "jsx" | "ts" | "tsx" => Some(Language::JsTs(javascript::JsTsParser)),
        "go" => Some(Language::Go(go::GoParser)),
        "py" => Some(Language::Python(python::PythonParser)),
        "cpp" | "cc" | "cxx" | "h" | "hpp" => Some(Language::Cpp(cpp::CppParser)),
        "c" => Some(Language::Cpp(cpp::CppParser)),
        "java" => Some(Language::JavaKotlin(java_kotlin::JavaKotlinParser)),
        "kt" | "kts" => Some(Language::JavaKotlin(java_kotlin::JavaKotlinParser)),
        "rb" => Some(Language::Ruby(ruby::RubyParser)),
        "php" => Some(Language::Php(php::PhpParser)),
        "swift" => Some(Language::Swift(swift::SwiftParser)),
        _ => None,
    }
}

pub struct ASTParser;

impl ASTParser {
    pub fn parse_file(path: &Path, content: &str) -> FileAnalysis {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let lang = match language_from_ext(ext) {
            Some(l) => l,
            None => return FileAnalysis::empty(),
        };
        let file_path = path.to_string_lossy().to_string();
        lang.parse(content, &file_path)
    }
}

pub struct TraverseContext<'a> {
    pub source: &'a [u8],
    pub file_path: &'a str,
    pub nodes: &'a mut Vec<NodeData>,
    pub edges: &'a mut Vec<EdgeData>,
    pub imports: &'a mut Vec<RawImport>,
    pub calls: &'a mut Vec<RawCall>,
}

pub fn extract_signature(node: Node, source: &[u8]) -> String {
    let mut start_byte = node.start_byte();
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            let child = cursor.node();
            if child.kind() == "decorator" {
                let end_byte = child.end_byte();
                if end_byte > start_byte {
                    start_byte = end_byte;
                }
            } else {
                break;
            }
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
    while start_byte < source.len() && source[start_byte].is_ascii_whitespace() {
        start_byte += 1;
    }
    let mut end_line_byte = start_byte;
    while end_line_byte < source.len() && source[end_line_byte] != b'\n' {
        end_line_byte += 1;
    }
    String::from_utf8_lossy(&source[start_byte..end_line_byte])
        .trim()
        .to_string()
}
