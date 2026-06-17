use crate::parser::{extract_signature, LanguageParser, TraverseContext};
use crate::types::ast::{EdgeData, FileAnalysis, NodeData, RawCall, RawImport};
use tree_sitter::{Node, Parser};

pub struct SwiftParser;

impl LanguageParser for SwiftParser {
    fn parse(&self, content: &str, file_path: &str) -> FileAnalysis {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        let mut imports = Vec::new();
        let mut calls = Vec::new();

        let mut parser = Parser::new();
        if parser.set_language(&tree_sitter_swift::language()).is_err() {
            return FileAnalysis {
                nodes,
                edges,
                imports,
                calls,
            };
        }
        crate::parser::apply_timeout(&mut parser);
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

fn traverse(node: Node, ctx: &mut TraverseContext, current_parent_id: Option<String>) {
    let kind = node.kind();
    let mut active_parent = current_parent_id;
    let start_point = node.start_position();
    let end_point = node.end_position();

    match kind {
        "import_declaration" => {
            // Extract module name from the identifier child
            for i in 0..node.child_count() {
                let child = node.child(i).unwrap();
                if child.kind() == "identifier" {
                    let module = child.utf8_text(ctx.source).unwrap_or("").to_owned();
                    if !module.is_empty() {
                        ctx.imports.push(RawImport {
                            path: module,
                            line: start_point.row + 1,
                        });
                    }
                }
            }
        }
        "call_expression" => {
            // call_expression has no fields in tree-sitter-swift 0.5.0;
            // structure is: <_expression> <call_suffix>
            if let Some(func_node) = node.child(0) {
                match func_node.kind() {
                    "simple_identifier" => {
                        let name = func_node.utf8_text(ctx.source).unwrap_or("").to_owned();
                        let is_valid = !name.is_empty()
                            && name.chars().all(|c| c.is_alphanumeric() || c == '_');
                        if is_valid {
                            ctx.calls.push(RawCall {
                                name,
                                line: start_point.row + 1,
                                is_method: false,
                            });
                        }
                    }
                    "navigation_expression" => {
                        // For `target.method()`, pull `method` out of
                        // navigation_expression.suffix = navigation_suffix(suffix = method).
                        if let Some(nav_suffix) = func_node.child_by_field_name("suffix") {
                            if let Some(member_node) = nav_suffix.child_by_field_name("suffix") {
                                let name =
                                    member_node.utf8_text(ctx.source).unwrap_or("").to_owned();
                                let is_valid = !name.is_empty()
                                    && name.chars().all(|c| c.is_alphanumeric() || c == '_');
                                if is_valid {
                                    ctx.calls.push(RawCall {
                                        name,
                                        line: start_point.row + 1,
                                        is_method: true,
                                    });
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        "function_declaration" => {
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

            let kind_label = if active_parent.is_some() {
                "Method"
            } else {
                "Function"
            };

            let signature = extract_signature(node, ctx.source);

            ctx.nodes.push(NodeData {
                id: symbol_id.clone(),
                name,
                kind: kind_label.to_owned(),
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
        "class_declaration"
        | "struct_declaration"
        | "enum_declaration"
        | "protocol_declaration" => {
            let name = if let Some(name_node) = node.child_by_field_name("name") {
                name_node
                    .utf8_text(ctx.source)
                    .unwrap_or("anonymous")
                    .to_owned()
            } else {
                "anonymous".to_owned()
            };

            // Determine kind from declaration_kind field or check for extension keyword
            let is_extension =
                (0..node.child_count()).any(|i| node.child(i).unwrap().kind() == "extension");

            let kind_label = if is_extension {
                "Extension"
            } else if let Some(decl_kind_node) = node.child_by_field_name("declaration_kind") {
                match decl_kind_node.utf8_text(ctx.source).unwrap_or("") {
                    "class" => "Class",
                    "struct" => "Struct",
                    "enum" => "Enum",
                    "protocol" => "Protocol",
                    _ => "Unknown",
                }
            } else {
                // Fallback to node kind
                match kind {
                    "class_declaration" => "Class",
                    "struct_declaration" => "Struct",
                    "enum_declaration" => "Enum",
                    "protocol_declaration" => "Protocol",
                    _ => "Unknown",
                }
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
                kind: kind_label.to_owned(),
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
#[allow(clippy::expect_used)] // test fixtures assert expected nodes
mod tests {
    use super::*;
    use crate::parser::ASTParser;
    use std::path::Path;

    #[test]
    fn test_swift_parsing() {
        let code = r#"
import Foundation
import UIKit

class ViewController: UIViewController {
    override func viewDidLoad() {
        super.viewDidLoad()
        loadData()
    }

    func loadData() {
        let data = fetchData()
        process(data)
    }
}

struct User {
    let name: String
    let email: String
}

enum Color {
    case red
    case green
    case blue
}

protocol Cacheable {
    func cacheKey() -> String
}

extension User {
    func displayName() -> String {
        return name
    }
}

func topLevelFunction() {
    let user = User(name: "test", email: "test@test.com")
    print(user.name)
}
        "#;
        let path = Path::new("test.swift");
        let FileAnalysis {
            nodes,
            edges: _,
            imports,
            calls,
        } = ASTParser::parse_file(path, code);

        assert!(!nodes.is_empty(), "Swift nodes should not be empty");
        assert!(imports.iter().any(|i| i.path == "Foundation"));
        assert!(imports.iter().any(|i| i.path == "UIKit"));

        let class_node = nodes
            .iter()
            .find(|n| n.name == "ViewController")
            .expect("ViewController class not found");
        assert_eq!(class_node.kind, "Class");

        let struct_node = nodes
            .iter()
            .find(|n| n.name == "User")
            .expect("User struct not found");
        assert_eq!(struct_node.kind, "Struct");

        let enum_node = nodes
            .iter()
            .find(|n| n.name == "Color")
            .expect("Color enum not found");
        assert_eq!(enum_node.kind, "Enum");

        let protocol_node = nodes
            .iter()
            .find(|n| n.name == "Cacheable")
            .expect("Cacheable protocol not found");
        assert_eq!(protocol_node.kind, "Protocol");

        let method_node = nodes
            .iter()
            .find(|n| n.name == "viewDidLoad")
            .expect("viewDidLoad method not found");
        assert_eq!(method_node.kind, "Method");

        let func_node = nodes
            .iter()
            .find(|n| n.name == "topLevelFunction")
            .expect("topLevelFunction not found");
        assert_eq!(func_node.kind, "Function");

        let ext_node = nodes
            .iter()
            .find(|n| n.name == "User" && n.kind == "Extension")
            .expect("User extension not found");
        assert_eq!(ext_node.kind, "Extension");

        let ext_method = nodes
            .iter()
            .find(|n| n.name == "displayName")
            .expect("displayName method not found in extension");
        assert_eq!(ext_method.kind, "Method");

        // Call-graph regression assertions
        // simple_identifier calls (no receiver): is_method=false
        assert!(
            calls.iter().any(|c| c.name == "loadData" && !c.is_method),
            "loadData() should be captured as a non-method call, got: {:?}",
            calls
        );
        assert!(
            calls.iter().any(|c| c.name == "fetchData" && !c.is_method),
            "fetchData() should be captured as a non-method call, got: {:?}",
            calls
        );
        // navigation_expression call (super.viewDidLoad): is_method=true
        assert!(
            calls.iter().any(|c| c.name == "viewDidLoad" && c.is_method),
            "super.viewDidLoad() should be captured with is_method=true, got: {:?}",
            calls
        );
        // Property access (user.name) must NOT be captured as a call —
        // it appears as an argument to print() but should not be promoted.
        assert!(
            !calls.iter().any(|c| c.name == "name" && c.is_method),
            "property access user.name should not be captured as a method call, got: {:?}",
            calls
        );
    }
}
