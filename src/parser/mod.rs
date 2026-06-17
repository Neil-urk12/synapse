pub mod cpp;
pub mod go;
pub mod java_kotlin;
pub mod javascript;
pub mod php;
pub mod python;
pub mod ruby;
pub mod rust;
pub mod swift;

/// Maximum source-file size accepted by the indexer. Files larger than this
/// are skipped with a warning (see `processor::process_file`) and become
/// `File` nodes with no `Symbol`s. Matches the threshold used by the peer
/// project pi-shazam (`MAX_PARSE_SIZE` in their `core/treesitter.ts`), which
/// added the cap in response to issue #101 (minified bundles / data files).
pub const MAX_PARSE_BYTES: u64 = 5 * 1024 * 1024;

/// Per-parse wall-clock budget in microseconds, enforced via
/// `tree_sitter::Parser::set_timeout_micros`. If a parse exceeds this limit
/// tree-sitter returns a best-effort partial tree; if that is unusable the
/// existing `parser.parse(_, _) -> None` path yields an empty `FileAnalysis`.
pub const PARSE_TIMEOUT_MICROS: u64 = 10_000_000;

/// Apply [`PARSE_TIMEOUT_MICROS`] to a freshly-initialized tree-sitter parser.
/// Each language parser in this module calls this immediately after
/// `set_language` so the threshold is set in one place.
pub fn apply_timeout(parser: &mut tree_sitter::Parser) {
    parser.set_timeout_micros(PARSE_TIMEOUT_MICROS);
}

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
        "cpp" | "cc" | "cxx" | "h" | "hpp" | "c" => Some(Language::Cpp(cpp::CppParser)),
        "java" | "kt" | "kts" => Some(Language::JavaKotlin(java_kotlin::JavaKotlinParser)),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Tripwire for the parser safety guards. Catches a future change that
    /// effectively disables the guard — either by setting it too low (no
    /// real source file would fit) or too high (minified bundles / data
    /// files would slip through). The `expect` strings are intentionally
    /// terse so the test output is self-explanatory.
    // The `allow` is intentional: this test exists specifically to verify
    // that the consts remain in a valid range. Clippy flags the comparison
    // as a constant assertion, but the tripwire fires only if someone later
    // changes the values to invalid ones (e.g. MAX_PARSE_BYTES = 0).
    #[allow(clippy::assertions_on_constants)]
    #[test]
    fn test_parser_limits_constants_are_sane() {
        assert!(
            MAX_PARSE_BYTES >= (1024 * 1024) as u64,
            "MAX_PARSE_BYTES must be at least 1 MiB to allow normal source files"
        );

        // Upper bound: 100 MiB. Anything larger stops catching the minified
        // bundles / data files the guard exists to skip.
        assert!(
            MAX_PARSE_BYTES <= 100 * 1024 * 1024,
            "MAX_PARSE_BYTES must be <= 100 MiB or the cap is effectively disabled"
        );
        assert!(
            PARSE_TIMEOUT_MICROS > 0,
            "PARSE_TIMEOUT_MICROS must be non-zero or the parse timeout is disabled"
        );
    }
}
