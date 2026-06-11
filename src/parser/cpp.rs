use crate::parser::{extract_signature, LanguageParser, TraverseContext};
use crate::types::ast::{EdgeData, FileAnalysis, NodeData, RawCall, RawImport};
use tree_sitter::{Node, Parser};

pub struct CppParser;

impl LanguageParser for CppParser {
    fn parse(&self, content: &str, file_path: &str) -> FileAnalysis {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        let mut imports = Vec::new();
        let mut calls = Vec::new();

        let lang = if file_path.ends_with(".c") || file_path.ends_with(".h") {
            tree_sitter_c::language()
        } else {
            tree_sitter_cpp::language()
        };

        let mut parser = Parser::new();
        if parser.set_language(&lang).is_err() {
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
    let mut active_parent = current_parent_id.clone();
    let start_point = node.start_position();
    let end_point = node.end_position();

    match kind {
        "preproc_include" => {
            if let Some(path_node) = node.child(1) {
                let path = path_node
                    .utf8_text(ctx.source)
                    .unwrap_or("")
                    .trim_matches(|c| c == '<' || c == '>' || c == '"')
                    .to_string();
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
                let mut name = func_node.utf8_text(ctx.source).unwrap_or("").to_string();
                if func_node.kind() == "field_expression" {
                    if let Some(field_node) = func_node.child_by_field_name("field") {
                        let method_name =
                            field_node.utf8_text(ctx.source).unwrap_or("").to_string();
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
                } else if func_node.kind() == "pointer_expression" {
                    let mut inner_cursor = func_node.walk();
                    if inner_cursor.goto_first_child() {
                        loop {
                            let c_node = inner_cursor.node();
                            if c_node.kind() == "field" {
                                let method_name =
                                    c_node.utf8_text(ctx.source).unwrap_or("").to_string();
                                ctx.calls.push(RawCall {
                                    name: method_name,
                                    line: start_point.row + 1,
                                    is_method: true,
                                });
                                break;
                            }
                            if !inner_cursor.goto_next_sibling() {
                                break;
                            }
                        }
                    }
                } else {
                    if name.contains("::") {
                        if let Some(last_segment) = name.split("::").last() {
                            name = last_segment.to_string();
                        }
                    }
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
            }
        }
        "class_specifier" | "struct_specifier" | "namespace_definition" => {
            let name = if let Some(name_node) = node.child_by_field_name("name") {
                name_node
                    .utf8_text(ctx.source)
                    .unwrap_or("anonymous")
                    .to_string()
            } else {
                "anonymous".to_string()
            };

            let kind_label = match kind {
                "class_specifier" => "Class",
                "struct_specifier" => "Struct",
                "namespace_definition" => "Namespace",
                _ => "Symbol",
            };

            let symbol_id = if let Some(ref parent) = current_parent_id {
                format!("{}::{}", parent, name)
            } else {
                format!("{}::{}", ctx.file_path, name)
            };

            let signature = extract_signature(node, ctx.source);

            ctx.nodes.push(NodeData {
                id: symbol_id.clone(),
                name,
                kind: kind_label.to_string(),
                start_line: start_point.row + 1,
                start_col: start_point.column + 1,
                end_line: end_point.row + 1,
                signature,
            });

            let from_id = current_parent_id
                .as_deref()
                .unwrap_or(ctx.file_path)
                .to_string();
            ctx.edges.push(EdgeData {
                from_id,
                to_id: symbol_id.clone(),
                edge_type: "CONTAINS".to_string(),
            });

            active_parent = Some(symbol_id);
        }
        "function_definition" => {
            let mut name = "anonymous".to_string();
            let mut go_parent = current_parent_id.clone();

            if let Some(declarator) = node.child_by_field_name("declarator") {
                let mut cursor = declarator.walk();
                loop {
                    let current_n = cursor.node();
                    if current_n.kind() == "function_declarator" {
                        if let Some(decl) = current_n.child_by_field_name("declarator") {
                            if decl.kind() == "qualified_identifier" {
                                let qual_text = decl.utf8_text(ctx.source).unwrap_or("");
                                if let Some(ns_sep_idx) = qual_text.rfind("::") {
                                    let class_part = &qual_text[..ns_sep_idx];
                                    let method_part = &qual_text[ns_sep_idx + 2..];
                                    name = method_part.to_string();
                                    go_parent =
                                        Some(format!("{}::{}", ctx.file_path, class_part));
                                } else {
                                    name = qual_text.to_string();
                                }
                            } else {
                                name = decl
                                    .utf8_text(ctx.source)
                                    .unwrap_or("anonymous")
                                    .to_string();
                            }
                        }
                        break;
                    }
                    if !cursor.goto_first_child() {
                        break;
                    }
                }
            }

            let mut is_method = false;
            if let Some(ref parent) = go_parent {
                if let Some(parent_node) = ctx.nodes.iter().find(|n| &n.id == parent) {
                    if parent_node.kind == "Class" || parent_node.kind == "Struct" {
                        is_method = true;
                    }
                }
            }
            let kind_label = if is_method { "Method" } else { "Function" };

            let symbol_id = if let Some(ref parent) = go_parent {
                format!("{}::{}", parent, name)
            } else {
                format!("{}::{}", ctx.file_path, name)
            };

            let signature = extract_signature(node, ctx.source);

            ctx.nodes.push(NodeData {
                id: symbol_id.clone(),
                name,
                kind: kind_label.to_string(),
                start_line: start_point.row + 1,
                start_col: start_point.column + 1,
                end_line: end_point.row + 1,
                signature,
            });

            let from_id = go_parent.unwrap_or_else(|| ctx.file_path.to_string());
            ctx.edges.push(EdgeData {
                from_id,
                to_id: symbol_id.clone(),
                edge_type: "CONTAINS".to_string(),
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
    fn test_cpp_parsing() {
        let code = r#"
            #include "helper.h"
            namespace ns {
                class Runner {
                public:
                    void run() {}
                };
            }
            int main() {
                ns::Runner r;
                r.run();
                return 0;
            }
        "#;
        let path = Path::new("test.cpp");
        let FileAnalysis {
            nodes,
            edges: _,
            imports,
            calls,
        } = ASTParser::parse_file(path, code);

        assert!(!nodes.is_empty(), "C++ nodes should not be empty");
        assert!(imports.iter().any(|i| i.path == "helper.h"));

        let ns_node = nodes
            .iter()
            .find(|n| n.name == "ns")
            .expect("Namespace ns not found");
        assert_eq!(ns_node.kind, "Namespace");

        let runner_node = nodes
            .iter()
            .find(|n| n.name == "Runner")
            .expect("Runner class not found");
        assert_eq!(runner_node.kind, "Class");

        let run_node = nodes
            .iter()
            .find(|n| n.name == "run")
            .expect("run method not found");
        assert_eq!(run_node.kind, "Method");

        assert!(calls.iter().any(|c| c.name == "run" && c.is_method));
    }
}
