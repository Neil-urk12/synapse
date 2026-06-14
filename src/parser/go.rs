use crate::parser::{extract_signature, LanguageParser, TraverseContext};
use crate::types::ast::{EdgeData, FileAnalysis, NodeData, RawCall, RawImport};
use tree_sitter::{Node, Parser};

pub struct GoParser;

impl LanguageParser for GoParser {
    fn parse(&self, content: &str, file_path: &str) -> FileAnalysis {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        let mut imports = Vec::new();
        let mut calls = Vec::new();

        let mut parser = Parser::new();
        if parser.set_language(&tree_sitter_go::language()).is_err() {
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

fn traverse(node: Node, ctx: &mut TraverseContext, current_parent_id: Option<String>) {
    let kind = node.kind();
    let mut active_parent = current_parent_id;
    let start_point = node.start_position();
    let end_point = node.end_position();

    match kind {
        "import_spec" => {
            if let Some(path_node) = node.child_by_field_name("path") {
                let path = path_node
                    .utf8_text(ctx.source)
                    .unwrap_or("")
                    .trim_matches(|c| c == '\'' || c == '"')
                    .to_owned();
                if !path.is_empty() {
                    ctx.imports.push(RawImport {
                        path,
                        line: start_point.row + 1,
                    });
                }
            }
        }
        "call_expression" => {
            if let Some(func_node) = node.child_by_field_name("function") {
                let name = func_node.utf8_text(ctx.source).unwrap_or("").to_owned();
                if func_node.kind() == "selector_expression" {
                    if let Some(field_node) = func_node.child_by_field_name("field") {
                        let method_name = field_node.utf8_text(ctx.source).unwrap_or("").to_owned();
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
        "function_declaration" | "method_declaration" => {
            let name = if let Some(name_node) = node.child_by_field_name("name") {
                name_node
                    .utf8_text(ctx.source)
                    .unwrap_or("anonymous")
                    .to_owned()
            } else {
                "anonymous".to_owned()
            };

            let kind_label = if kind == "method_declaration" {
                "Method"
            } else {
                "Function"
            };
            let mut go_parent = active_parent.clone();
            if kind == "method_declaration" {
                if let Some(receiver_node) = node.child_by_field_name("receiver") {
                    let mut cursor = receiver_node.walk();
                    let mut found_type = None;
                    loop {
                        let r_child = cursor.node();
                        if r_child.kind() == "parameter_declaration" {
                            if let Some(type_node) = r_child.child_by_field_name("type") {
                                let type_text = type_node.utf8_text(ctx.source).unwrap_or("");
                                found_type =
                                    Some(type_text.trim_start_matches('*').trim().to_owned());
                                break;
                            }
                        }
                        if !cursor.goto_next_sibling() {
                            break;
                        }
                    }
                    if let Some(t_name) = found_type {
                        go_parent = Some(format!("{}::{}", ctx.file_path, t_name));
                    }
                }
            }

            let symbol_id = if let Some(ref parent) = go_parent {
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

            let from_id = go_parent.unwrap_or_else(|| ctx.file_path.to_owned());
            ctx.edges.push(EdgeData {
                from_id,
                to_id: symbol_id.clone(),
                edge_type: "CONTAINS".to_owned(),
            });

            active_parent = Some(symbol_id);
        }
        "type_declaration" => {
            let mut cursor = node.walk();
            if cursor.goto_first_child() {
                loop {
                    let child = cursor.node();
                    if child.kind() == "type_spec" {
                        if let Some(name_node) = child.child_by_field_name("name") {
                            let name = name_node
                                .utf8_text(ctx.source)
                                .unwrap_or("anonymous")
                                .to_owned();
                            if let Some(type_node) = child.child_by_field_name("type") {
                                let kind_label = match type_node.kind() {
                                    "struct_type" => "Struct",
                                    "interface_type" => "Interface",
                                    _ => "",
                                };
                                if !kind_label.is_empty() {
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

                                    let from_id = active_parent
                                        .as_deref()
                                        .unwrap_or(ctx.file_path)
                                        .to_owned();
                                    ctx.edges.push(EdgeData {
                                        from_id,
                                        to_id: symbol_id.clone(),
                                        edge_type: "CONTAINS".to_owned(),
                                    });

                                    active_parent = Some(symbol_id);
                                }
                            }
                        }
                    }
                    if !cursor.goto_next_sibling() {
                        break;
                    }
                }
            }
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
    fn test_go_parsing() {
        let code = r#"
            package main
            import (
                "fmt"
                "math"
            )
            type MyStruct struct {
                val int
            }
            type MyInterface interface {
                DoWork()
            }
            func (m *MyStruct) Process(x int) {
                fmt.Println(x)
            }
            func main() {
                var s MyStruct
                s.Process(10)
            }
        "#;
        let path = Path::new("test.go");
        let FileAnalysis {
            nodes,
            edges: _,
            imports,
            calls,
        } = ASTParser::parse_file(path, code);

        assert!(!nodes.is_empty(), "Go nodes should not be empty");
        assert!(imports.iter().any(|i| i.path == "fmt"));
        assert!(imports.iter().any(|i| i.path == "math"));

        let process_node = nodes
            .iter()
            .find(|n| n.name == "Process")
            .expect("Process method not found");
        assert_eq!(process_node.kind, "Method");

        let struct_node = nodes
            .iter()
            .find(|n| n.name == "MyStruct")
            .expect("MyStruct struct not found");
        assert_eq!(struct_node.kind, "Struct");

        assert!(calls.iter().any(|c| c.name == "Process" && c.is_method));
    }
}
