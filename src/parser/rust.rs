use crate::parser::{extract_signature, LanguageParser, TraverseContext};
use crate::types::ast::{EdgeData, FileAnalysis, NodeData, RawCall, RawImport};
use tree_sitter::{Node, Parser};

pub struct RustParser;

impl LanguageParser for RustParser {
    fn parse(&self, content: &str, file_path: &str) -> FileAnalysis {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        let mut imports = Vec::new();
        let mut calls = Vec::new();

        let mut parser = Parser::new();
        if parser.set_language(&tree_sitter_rust::language()).is_err() {
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
        "use_declaration" => {
            if let Ok(text) = node.utf8_text(ctx.source) {
                let trimmed = text.trim().trim_end_matches(';').trim();
                if let Some(use_idx) = trimmed.find("use ") {
                    let path = trimmed[use_idx + 4..].trim().to_string();
                    for expanded in expand_rust_import(&path) {
                        ctx.imports.push(RawImport {
                            path: expanded,
                            line: start_point.row + 1,
                        });
                    }
                }
            }
        }
        "method_call_expression" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = name_node.utf8_text(ctx.source).unwrap_or("").to_string();
                let is_valid = !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == '_' || c == '$' || c == ':');
                if is_valid {
                    ctx.calls.push(RawCall {
                        name,
                        line: start_point.row + 1,
                        is_method: true,
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
                            && method_name
                                .chars()
                                .all(|c| c.is_alphanumeric() || c == '_' || c == '$' || c == ':');
                        if is_valid {
                            ctx.calls.push(RawCall {
                                name: method_name,
                                line: start_point.row + 1,
                                is_method: true,
                            });
                        }
                    }
                } else {
                    if name.contains("::") {
                        if let Some(last_segment) = name.split("::").last() {
                            name = last_segment.to_string();
                        }
                    }
                    let is_valid = !name.is_empty()
                        && name != "require"
                        && name
                            .chars()
                            .all(|c| c.is_alphanumeric() || c == '_' || c == '$' || c == ':');
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
        "function_item"
        | "struct_item"
        | "enum_item"
        | "trait_item"
        | "impl_item" => {
            let name = if let Some(name_node) = node.child_by_field_name("name") {
                name_node
                    .utf8_text(ctx.source)
                    .unwrap_or("anonymous")
                    .to_string()
            } else if kind == "impl_item" {
                let type_name = if let Some(type_node) = node.child_by_field_name("type") {
                    type_node.utf8_text(ctx.source).unwrap_or("Type")
                } else {
                    "Type"
                };
                if let Some(trait_node) = node.child_by_field_name("trait") {
                    let trait_name = trait_node.utf8_text(ctx.source).unwrap_or("Trait");
                    format!("impl {} for {}", trait_name, type_name)
                } else {
                    format!("impl {}", type_name)
                }
            } else {
                "anonymous".to_string()
            };

            let kind_label = match kind {
                "function_item" => "Function",
                "struct_item" => "Struct",
                "enum_item" => "Enum",
                "trait_item" => "Interface",
                "impl_item" => "Implementation",
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

pub fn expand_rust_import(path: &str) -> Vec<String> {
    let path = path.trim();
    if let Some(brace_idx) = path.find('{') {
        let prefix = &path[..brace_idx];
        let mut depth = 0;
        let mut closing_idx = None;
        for (i, c) in path[brace_idx..].char_indices() {
            if c == '{' {
                depth += 1;
            } else if c == '}' {
                depth -= 1;
                if depth == 0 {
                    closing_idx = Some(brace_idx + i);
                    break;
                }
            }
        }

        if let Some(end_idx) = closing_idx {
            let inner = &path[brace_idx + 1..end_idx];
            let suffix = &path[end_idx + 1..];

            let mut parts = Vec::new();
            let mut current = String::new();
            let mut inner_depth = 0;
            for c in inner.chars() {
                if c == ',' && inner_depth == 0 {
                    parts.push(current.trim().to_string());
                    current.clear();
                } else {
                    if c == '{' {
                        inner_depth += 1;
                    } else if c == '}' {
                        inner_depth -= 1;
                    }
                    current.push(c);
                }
            }
            if !current.trim().is_empty() {
                parts.push(current.trim().to_string());
            }

            let mut results = Vec::new();
            for part in parts {
                let combined_prefix = if prefix.ends_with("::") || part.starts_with("::") {
                    format!("{}{}", prefix, part)
                } else {
                    format!("{}::{}", prefix, part)
                };

                let combined = if suffix.starts_with("::") || suffix.is_empty() {
                    format!("{}{}", combined_prefix, suffix)
                } else {
                    format!("{}::{}", combined_prefix, suffix)
                };

                results.extend(expand_rust_import(&combined));
            }
            results
        } else {
            vec![path.to_string()]
        }
    } else {
        let clean_path = if let Some(as_idx) = path.find(" as ") {
            path[..as_idx].trim().to_string()
        } else {
            path.to_string()
        };
        vec![clean_path]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::ASTParser;
    use std::path::Path;

    fn node_by_id<'a>(nodes: &'a [NodeData], id: &str) -> &'a NodeData {
        nodes.iter().find(|n| n.id == id).unwrap_or_else(|| {
            panic!(
                "node with id '{id}' not found in: {:#?}",
                nodes.iter().map(|n| &n.id).collect::<Vec<_>>()
            )
        })
    }

    fn edge_by_endpoints<'a>(edges: &'a [EdgeData], from: &str, to: &str) -> &'a EdgeData {
        edges
            .iter()
            .find(|e| e.from_id == from && e.to_id == to)
            .unwrap_or_else(|| {
                panic!(
                    "edge '{from}'->'{to}' not found in: {:#?}",
                    edges
                        .iter()
                        .map(|e| (&e.from_id, &e.to_id))
                        .collect::<Vec<_>>()
                )
            })
    }

    #[test]
    fn test_rust_declarations() {
        let code = r#"
            enum MyEnum {
                Variant,
            }
            trait MyTrait {
                fn trait_func() {}
            }
            struct MyStruct {
                x: i32,
            }
            impl MyStruct {
                #[inline]
                fn run() {}
            }
            impl MyTrait for MyStruct {
                fn trait_func() {}
            }
        "#;
        let path = Path::new("test.rs");
        let FileAnalysis { nodes, edges, .. } = ASTParser::parse_file(path, code);

        assert!(!nodes.is_empty(), "Nodes should not be empty");

        let node_names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
        let node_kinds: Vec<&str> = nodes.iter().map(|n| n.kind.as_str()).collect();

        assert_eq!(
            node_names,
            vec![
                "MyEnum",
                "MyTrait",
                "trait_func",
                "MyStruct",
                "impl MyStruct",
                "run",
                "impl MyTrait for MyStruct",
                "trait_func"
            ]
        );
        assert_eq!(
            node_kinds,
            vec![
                "Enum",
                "Interface",
                "Function",
                "Struct",
                "Implementation",
                "Function",
                "Implementation",
                "Function"
            ]
        );

        assert_eq!(node_by_id(&nodes, "test.rs::MyEnum").kind, "Enum");
        assert_eq!(node_by_id(&nodes, "test.rs::MyTrait").kind, "Interface");
        assert_eq!(
            node_by_id(&nodes, "test.rs::MyTrait::trait_func").kind,
            "Function"
        );
        assert_eq!(node_by_id(&nodes, "test.rs::MyStruct").kind, "Struct");
        assert_eq!(
            node_by_id(&nodes, "test.rs::impl MyStruct").kind,
            "Implementation"
        );
        assert_eq!(
            node_by_id(&nodes, "test.rs::impl MyStruct::run").kind,
            "Function"
        );
        assert_eq!(
            node_by_id(&nodes, "test.rs::impl MyTrait for MyStruct").kind,
            "Implementation"
        );
        assert_eq!(
            node_by_id(&nodes, "test.rs::impl MyTrait for MyStruct::trait_func").kind,
            "Function"
        );

        assert_eq!(edges.len(), 8);
        assert_eq!(
            edge_by_endpoints(&edges, "test.rs", "test.rs::MyEnum").edge_type,
            "CONTAINS"
        );
        edge_by_endpoints(&edges, "test.rs", "test.rs::MyTrait");
        edge_by_endpoints(&edges, "test.rs::MyTrait", "test.rs::MyTrait::trait_func");
        edge_by_endpoints(&edges, "test.rs", "test.rs::MyStruct");
        edge_by_endpoints(&edges, "test.rs", "test.rs::impl MyStruct");
        edge_by_endpoints(
            &edges,
            "test.rs::impl MyStruct",
            "test.rs::impl MyStruct::run",
        );
        edge_by_endpoints(&edges, "test.rs", "test.rs::impl MyTrait for MyStruct");
        edge_by_endpoints(
            &edges,
            "test.rs::impl MyTrait for MyStruct",
            "test.rs::impl MyTrait for MyStruct::trait_func",
        );
    }

    #[test]
    fn test_rust_parsing() {
        let code = r#"
            pub use crate::db::Connection;
            pub(crate) use std::path::Path;

            fn call_helper() {
                do_something();
            }

            struct Runner;
            impl Runner {
                fn run(&self) {
                    self.execute();
                }
            }
        "#;
        let path = Path::new("test.rs");
        let analysis = ASTParser::parse_file(path, code);

        assert_eq!(analysis.imports.len(), 2);
        assert_eq!(analysis.imports[0].path, "crate::db::Connection");
        assert_eq!(analysis.imports[1].path, "std::path::Path");

        assert_eq!(analysis.calls.len(), 2);
        assert_eq!(analysis.calls[0].name, "do_something");
        assert!(!analysis.calls[0].is_method);
        assert_eq!(analysis.calls[1].name, "execute");
        assert!(analysis.calls[1].is_method);
    }

    #[test]
    fn test_expand_rust_import() {
        assert_eq!(
            expand_rust_import("std::collections::{HashMap, HashSet}"),
            vec!["std::collections::HashMap", "std::collections::HashSet"]
        );
        assert_eq!(
            expand_rust_import("crate::{parser::ASTParser, linker::run_linker}"),
            vec!["crate::parser::ASTParser", "crate::linker::run_linker"]
        );
        assert_eq!(
            expand_rust_import("self::foo::{bar as baz, qux}"),
            vec!["self::foo::bar", "self::foo::qux"]
        );
        assert_eq!(
            expand_rust_import("crate::{a::{b, c}, d}"),
            vec!["crate::a::b", "crate::a::c", "crate::d"]
        );
    }
}
