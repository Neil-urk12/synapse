use crate::parser::{extract_signature, LanguageParser, TraverseContext};
use crate::types::ast::{EdgeData, FileAnalysis, NodeData, RawCall, RawImport};
use tree_sitter::{Node, Parser};

pub struct PythonParser;

impl LanguageParser for PythonParser {
    fn parse(&self, content: &str, file_path: &str) -> FileAnalysis {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        let mut imports = Vec::new();
        let mut calls = Vec::new();

        let mut parser = Parser::new();
        if parser
            .set_language(&tree_sitter_python::language())
            .is_err()
        {
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
        "import_statement" => {
            let mut cursor = node.walk();
            if cursor.goto_first_child() {
                loop {
                    let child = cursor.node();
                    if child.kind() == "dotted_name" {
                        let path = child.utf8_text(ctx.source).unwrap_or("").to_owned();
                        if !path.is_empty() {
                            ctx.imports.push(RawImport {
                                path,
                                line: start_point.row + 1,
                            });
                        }
                    }
                    if !cursor.goto_next_sibling() {
                        break;
                    }
                }
            }
        }
        "import_from_statement" => {
            let mut module_path = String::new();
            let mut cursor = node.walk();
            if cursor.goto_first_child() {
                let mut is_after_from = false;
                let mut is_after_import = false;
                loop {
                    let child = cursor.node();
                    if child.kind() == "from" {
                        is_after_from = true;
                    } else if child.kind() == "import" {
                        is_after_import = true;
                        is_after_from = false;
                    } else if is_after_from {
                        if child.kind() == "dotted_name" || child.kind() == "relative_import" {
                            module_path = child.utf8_text(ctx.source).unwrap_or("").to_owned();
                            if !module_path.is_empty() {
                                ctx.imports.push(RawImport {
                                    path: module_path.clone(),
                                    line: start_point.row + 1,
                                });
                            }
                        }
                    } else if is_after_import
                        && (child.kind() == "dotted_name"
                            || child.kind() == "identifier"
                            || child.kind() == "aliased_import")
                    {
                        let mut name_text = child.utf8_text(ctx.source).unwrap_or("").to_owned();
                        if child.kind() == "aliased_import" {
                            if let Some(name_node) = child.child_by_field_name("name") {
                                name_text =
                                    name_node.utf8_text(ctx.source).unwrap_or("").to_owned();
                            }
                        }
                        if !name_text.is_empty() && !module_path.is_empty() {
                            ctx.imports.push(RawImport {
                                path: format!("{}::{}", module_path, name_text),
                                line: start_point.row + 1,
                            });
                        }
                    }
                    if !cursor.goto_next_sibling() {
                        break;
                    }
                }
            }
        }
        "call" => {
            if let Some(func_node) = node.child_by_field_name("function") {
                let name = func_node.utf8_text(ctx.source).unwrap_or("").to_owned();
                if func_node.kind() == "attribute" {
                    if let Some(attribute_node) = func_node.child_by_field_name("attribute") {
                        let method_name = attribute_node
                            .utf8_text(ctx.source)
                            .unwrap_or("")
                            .to_owned();
                        let is_valid = !method_name.is_empty()
                            && method_name.chars().all(|c| c.is_alphanumeric() || c == '_');
                        if is_valid {
                            ctx.calls.push(RawCall {
                                name: method_name,
                                line: start_point.row + 1,
                                is_method: true,
                            });
                        }
                    }
                } else {
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
        }
        "class_definition" => {
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
        "function_definition" => {
            let name = if let Some(name_node) = node.child_by_field_name("name") {
                name_node
                    .utf8_text(ctx.source)
                    .unwrap_or("anonymous")
                    .to_owned()
            } else {
                "anonymous".to_owned()
            };

            let mut is_method = false;
            if let Some(ref parent) = active_parent {
                if let Some(parent_node) = ctx.nodes.iter().find(|n| &n.id == parent) {
                    if parent_node.kind == "Class" {
                        is_method = true;
                    }
                }
            }

            let kind_label = if is_method { "Method" } else { "Function" };

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
    fn test_python_parsing() {
        let code = r#"
import os
from datetime import datetime

class Helper:
    def greet(self):
        print("Hello")

def run():
    h = Helper()
    h.greet()
        "#;
        let path = Path::new("test.py");
        let FileAnalysis {
            nodes,
            edges: _,
            imports,
            calls,
        } = ASTParser::parse_file(path, code);

        assert!(!nodes.is_empty(), "Python nodes should not be empty");
        assert!(imports.iter().any(|i| i.path == "os"));
        assert!(imports.iter().any(|i| i.path == "datetime"));

        let class_node = nodes
            .iter()
            .find(|n| n.name == "Helper")
            .expect("Helper class not found");
        assert_eq!(class_node.kind, "Class");

        let greet_node = nodes
            .iter()
            .find(|n| n.name == "greet")
            .expect("greet method not found");
        assert_eq!(greet_node.kind, "Method");

        let run_node = nodes
            .iter()
            .find(|n| n.name == "run")
            .expect("run function not found");
        assert_eq!(run_node.kind, "Function");

        assert!(calls.iter().any(|c| c.name == "greet" && c.is_method));
    }
}
