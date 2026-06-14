use crate::parser::{extract_signature, LanguageParser, TraverseContext};
use crate::types::ast::{EdgeData, FileAnalysis, NodeData, RawCall, RawImport};
use tree_sitter::{Node, Parser};

pub struct PhpParser;

impl LanguageParser for PhpParser {
    fn parse(&self, content: &str, file_path: &str) -> FileAnalysis {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        let mut imports = Vec::new();
        let mut calls = Vec::new();

        let mut parser = Parser::new();
        if parser
            .set_language(&tree_sitter_php::language_php())
            .is_err()
        {
            return FileAnalysis {
                nodes,
                edges,
                imports,
                calls,
            };
        }
        let tree = match parser.parse(content, None) {
            Some(t) => t,
            None => {
                return FileAnalysis {
                    nodes,
                    edges,
                    imports,
                    calls,
                }
            }
        };
        let source_bytes = content.as_bytes();

        let mut ctx = TraverseContext {
            source: source_bytes,
            file_path,
            nodes: &mut nodes,
            edges: &mut edges,
            imports: &mut imports,
            calls: &mut calls,
        };

        traverse(tree.root_node(), &mut ctx, None);

        FileAnalysis {
            nodes,
            edges,
            imports,
            calls,
        }
    }
}

fn extract_string_literal(node: Node, source: &[u8]) -> Option<String> {
    match node.kind() {
        "string" | "encapsed_string" => {
            let mut cursor = node.walk();
            if cursor.goto_first_child() {
                loop {
                    let child = cursor.node();
                    if child.kind() == "string_content" {
                        return Some(child.utf8_text(source).unwrap_or("").to_owned());
                    }
                    if !cursor.goto_next_sibling() {
                        break;
                    }
                }
            }
            None
        }
        "parenthesized_expression" => {
            let mut cursor = node.walk();
            if cursor.goto_first_child() {
                loop {
                    if let Some(path) = extract_string_literal(cursor.node(), source) {
                        return Some(path);
                    }
                    if !cursor.goto_next_sibling() {
                        break;
                    }
                }
            }
            None
        }
        _ => None,
    }
}

fn extract_require_path(node: Node, source: &[u8], line: usize, imports: &mut Vec<RawImport>) {
    if let Some(arg_node) = node.child(1) {
        if let Some(path) = extract_string_literal(arg_node, source) {
            if !path.is_empty() {
                imports.push(RawImport { path, line });
            }
        }
    }
}

fn traverse(node: Node, ctx: &mut TraverseContext, current_parent_id: Option<String>) {
    let kind = node.kind();
    let mut active_parent = current_parent_id;
    let start_point = node.start_position();
    let end_point = node.end_position();

    match kind {
        "namespace_use_declaration" => {
            // Handle 'use' statements
            for i in 0..node.child_count() {
                let child = node.child(i).unwrap();
                if child.kind() == "namespace_use_clause" {
                    let path = child
                        .utf8_text(ctx.source)
                        .unwrap_or("")
                        .trim_matches(|c| c == ';' || c == ' ')
                        .to_owned();
                    if !path.is_empty() {
                        ctx.imports.push(RawImport {
                            path,
                            line: start_point.row + 1,
                        });
                    }
                }
            }
        }
        "require_expression"
        | "include_expression"
        | "require_once_expression"
        | "include_once_expression" => {
            extract_require_path(node, ctx.source, start_point.row + 1, ctx.imports);
        }
        "expression_statement" => {
            if let Some(child) = node.child(0) {
                if child.kind() == "require_expression"
                    || child.kind() == "include_expression"
                    || child.kind() == "require_once_expression"
                    || child.kind() == "include_once_expression"
                {
                    extract_require_path(child, ctx.source, start_point.row + 1, ctx.imports);
                }
            }
        }
        "function_call_expression" => {
            if let Some(func_node) = node.child_by_field_name("function") {
                let name = func_node.utf8_text(ctx.source).unwrap_or("").to_owned();
                let is_valid =
                    !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_');
                if is_valid {
                    ctx.calls.push(RawCall {
                        name,
                        line: start_point.row + 1,
                        is_method: false,
                    });
                }
            }
        }
        "member_call_expression" => {
            if let Some(func_node) = node.child_by_field_name("name") {
                let name = func_node.utf8_text(ctx.source).unwrap_or("").to_owned();
                let is_valid =
                    !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_');
                if is_valid {
                    ctx.calls.push(RawCall {
                        name,
                        line: start_point.row + 1,
                        is_method: true,
                    });
                }
            }
        }
        "scoped_call_expression" => {
            if let Some(func_node) = node.child_by_field_name("name") {
                let name = func_node.utf8_text(ctx.source).unwrap_or("").to_owned();
                let is_valid =
                    !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_');
                if is_valid {
                    ctx.calls.push(RawCall {
                        name,
                        line: start_point.row + 1,
                        is_method: true,
                    });
                }
            }
        }
        "function_definition" => {
            let name = if let Some(name_node) = node.child_by_field_name("name") {
                name_node
                    .utf8_text(ctx.source)
                    .unwrap_or("anonymous")
                    .to_owned()
            } else {
                "anonymous".to_owned()
            };

            let symbol_id = if let Some(ref parent) = active_parent {
                format!("{}::{}", parent, name)
            } else {
                format!("{}::{}", ctx.file_path, name)
            };

            let signature = extract_signature(node, ctx.source);

            ctx.nodes.push(NodeData {
                id: symbol_id.clone(),
                name,
                kind: "Function".to_owned(),
                start_line: start_point.row + 1,
                start_col: start_point.column + 1,
                end_line: end_point.row + 1,
                signature,
            });

            let from_id = active_parent.as_deref().unwrap_or(ctx.file_path).to_owned();
            ctx.edges.push(EdgeData {
                from_id,
                to_id: symbol_id.clone(),
                edge_type: "CONTAINS".to_owned(),
            });

            active_parent = Some(symbol_id);
        }
        "method_declaration" => {
            let name = if let Some(name_node) = node.child_by_field_name("name") {
                name_node
                    .utf8_text(ctx.source)
                    .unwrap_or("anonymous")
                    .to_owned()
            } else {
                "anonymous".to_owned()
            };

            let symbol_id = if let Some(ref parent) = active_parent {
                format!("{}::{}", parent, name)
            } else {
                format!("{}::{}", ctx.file_path, name)
            };

            let signature = extract_signature(node, ctx.source);

            ctx.nodes.push(NodeData {
                id: symbol_id.clone(),
                name,
                kind: "Method".to_owned(),
                start_line: start_point.row + 1,
                start_col: start_point.column + 1,
                end_line: end_point.row + 1,
                signature,
            });

            let from_id = active_parent.as_deref().unwrap_or(ctx.file_path).to_owned();
            ctx.edges.push(EdgeData {
                from_id,
                to_id: symbol_id.clone(),
                edge_type: "CONTAINS".to_owned(),
            });

            active_parent = Some(symbol_id);
        }
        "class_declaration" => {
            let name = if let Some(name_node) = node.child_by_field_name("name") {
                name_node
                    .utf8_text(ctx.source)
                    .unwrap_or("anonymous")
                    .to_owned()
            } else {
                "anonymous".to_owned()
            };

            let symbol_id = if let Some(ref parent) = active_parent {
                format!("{}::{}", parent, name)
            } else {
                format!("{}::{}", ctx.file_path, name)
            };

            let signature = extract_signature(node, ctx.source);

            ctx.nodes.push(NodeData {
                id: symbol_id.clone(),
                name,
                kind: "Class".to_owned(),
                start_line: start_point.row + 1,
                start_col: start_point.column + 1,
                end_line: end_point.row + 1,
                signature,
            });

            let from_id = active_parent.as_deref().unwrap_or(ctx.file_path).to_owned();
            ctx.edges.push(EdgeData {
                from_id,
                to_id: symbol_id.clone(),
                edge_type: "CONTAINS".to_owned(),
            });

            active_parent = Some(symbol_id);
        }
        "interface_declaration" => {
            let name = if let Some(name_node) = node.child_by_field_name("name") {
                name_node
                    .utf8_text(ctx.source)
                    .unwrap_or("anonymous")
                    .to_owned()
            } else {
                "anonymous".to_owned()
            };

            let symbol_id = if let Some(ref parent) = active_parent {
                format!("{}::{}", parent, name)
            } else {
                format!("{}::{}", ctx.file_path, name)
            };

            let signature = extract_signature(node, ctx.source);

            ctx.nodes.push(NodeData {
                id: symbol_id.clone(),
                name,
                kind: "Interface".to_owned(),
                start_line: start_point.row + 1,
                start_col: start_point.column + 1,
                end_line: end_point.row + 1,
                signature,
            });

            let from_id = active_parent.as_deref().unwrap_or(ctx.file_path).to_owned();
            ctx.edges.push(EdgeData {
                from_id,
                to_id: symbol_id.clone(),
                edge_type: "CONTAINS".to_owned(),
            });

            active_parent = Some(symbol_id);
        }
        "trait_declaration" => {
            let name = if let Some(name_node) = node.child_by_field_name("name") {
                name_node
                    .utf8_text(ctx.source)
                    .unwrap_or("anonymous")
                    .to_owned()
            } else {
                "anonymous".to_owned()
            };

            let symbol_id = if let Some(ref parent) = active_parent {
                format!("{}::{}", parent, name)
            } else {
                format!("{}::{}", ctx.file_path, name)
            };

            let signature = extract_signature(node, ctx.source);

            ctx.nodes.push(NodeData {
                id: symbol_id.clone(),
                name,
                kind: "Trait".to_owned(),
                start_line: start_point.row + 1,
                start_col: start_point.column + 1,
                end_line: end_point.row + 1,
                signature,
            });

            let from_id = active_parent.as_deref().unwrap_or(ctx.file_path).to_owned();
            ctx.edges.push(EdgeData {
                from_id,
                to_id: symbol_id.clone(),
                edge_type: "CONTAINS".to_owned(),
            });

            active_parent = Some(symbol_id);
        }
        _ => {}
    }

    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            traverse(cursor.node(), ctx, active_parent.clone());
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::ASTParser;
    use std::path::Path;

    #[test]
    fn test_php_parsing() {
        let code = r#"
<?php
require 'config.php';
use App\User;

class UserController {
    public function index() {
        $this->validate($data);
        return User::all();
    }

    private function validate($data) {
        return true;
    }
}

interface CacheInterface {
    public function get($key);
}

trait Cacheable {
    public function cacheKey() {
        return 'key';
    }
}

function helper() {
    echo "help";
}
?>
        "#;
        let path = Path::new("test.php");
        let FileAnalysis {
            nodes,
            edges: _,
            imports,
            calls,
        } = ASTParser::parse_file(path, code);

        assert!(!nodes.is_empty(), "PHP nodes should not be empty");
        assert!(imports.iter().any(|i| i.path == "config.php"));
        assert!(imports.iter().any(|i| i.path == "App\\User"));

        let class_node = nodes
            .iter()
            .find(|n| n.name == "UserController")
            .expect("UserController class not found");
        assert_eq!(class_node.kind, "Class");

        let interface_node = nodes
            .iter()
            .find(|n| n.name == "CacheInterface")
            .expect("CacheInterface interface not found");
        assert_eq!(interface_node.kind, "Interface");

        let trait_node = nodes
            .iter()
            .find(|n| n.name == "Cacheable")
            .expect("Cacheable trait not found");
        assert_eq!(trait_node.kind, "Trait");

        let method_node = nodes
            .iter()
            .find(|n| n.name == "index")
            .expect("index method not found");
        assert_eq!(method_node.kind, "Method");

        let func_node = nodes
            .iter()
            .find(|n| n.name == "helper")
            .expect("helper function not found");
        assert_eq!(func_node.kind, "Function");

        // Call-graph regression assertions
        assert!(
            calls.iter().any(|c| c.name == "validate" && c.is_method),
            "member call $this->validate(...) should be captured as a method call, got: {:?}",
            calls
        );
        assert!(
            calls.iter().any(|c| c.name == "all" && c.is_method),
            "static call User::all() should be captured as a method call, got: {:?}",
            calls
        );
    }

    #[test]
    fn test_php_require_variants() {
        let code = r#"<?php
require 'simple.php';
require('parens.php');
include_once "double.php";
require_once('once.php');
require $variable_path;
?>
"#;
        let path = Path::new("test.php");
        let FileAnalysis { imports, .. } = ASTParser::parse_file(path, code);

        let paths: Vec<&str> = imports.iter().map(|i| i.path.as_str()).collect();
        assert!(paths.contains(&"simple.php"), "simple form: {:?}", paths);
        assert!(
            paths.contains(&"parens.php"),
            "parenthesized form: {:?}",
            paths
        );
        assert!(
            paths.contains(&"double.php"),
            "double-quoted form: {:?}",
            paths
        );
        assert!(
            paths.contains(&"once.php"),
            "require_once form: {:?}",
            paths
        );
        assert!(
            !paths.iter().any(|p| p.contains('$') || p.contains('(')),
            "non-literal path should not be captured: {:?}",
            paths
        );
    }
}
