use crate::parser::{extract_signature, LanguageParser, TraverseContext};
use crate::types::ast::{EdgeData, FileAnalysis, NodeData, RawCall, RawImport};
use tree_sitter::{Node, Parser};

pub struct JsTsParser;

impl LanguageParser for JsTsParser {
    fn parse(&self, content: &str, file_path: &str) -> FileAnalysis {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        let mut imports = Vec::new();
        let mut calls = Vec::new();

        let lang = if file_path.ends_with(".tsx") {
            tree_sitter_typescript::language_tsx()
        } else if file_path.ends_with(".ts") {
            tree_sitter_typescript::language_typescript()
        } else {
            tree_sitter_javascript::language()
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
    let mut active_parent = current_parent_id;
    let start_point = node.start_position();
    let end_point = node.end_position();

    match kind {
        "import_statement" | "export_statement" => {
            if let Some(source_node) = node.child_by_field_name("source") {
                let path = source_node
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
                if name == "require" {
                    let mut cursor = node.walk();
                    if cursor.goto_first_child() {
                        loop {
                            let arg_node = cursor.node();
                            if arg_node.kind() == "arguments" {
                                let mut inner_cursor = arg_node.walk();
                                if inner_cursor.goto_first_child() {
                                    loop {
                                        let child = inner_cursor.node();
                                        if child.kind() == "string" {
                                            let path = child
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
                                            break;
                                        }
                                        if !inner_cursor.goto_next_sibling() {
                                            break;
                                        }
                                    }
                                }
                                break;
                            }
                            if !cursor.goto_next_sibling() {
                                break;
                            }
                        }
                    }
                } else if func_node.kind() == "member_expression" {
                    if let Some(prop_node) = func_node.child_by_field_name("property") {
                        let method_name =
                            prop_node.utf8_text(ctx.source).unwrap_or("").to_owned();
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
                    let is_valid = !name.is_empty()
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
        "function_declaration"
        | "class_declaration"
        | "method_definition"
        | "interface_declaration" => {
            let name = if let Some(name_node) = node.child_by_field_name("name") {
                name_node
                    .utf8_text(ctx.source)
                    .unwrap_or("anonymous")
                    .to_owned()
            } else {
                "anonymous".to_owned()
            };

            let kind_label = match kind {
                "function_declaration" => "Function",
                "class_declaration" => "Class",
                "method_definition" => "Method",
                "interface_declaration" => "Interface",
                _ => "Symbol",
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
    use crate::parser::ASTParser;
    use std::path::Path;

    #[test]
    fn test_js_parsing() {
        let code = r#"
            import { something } from './module.js';
            const fs = require('fs');

            class User {
                login() {
                    console.log("logged in");
                }
            }
            function register() {
                doJSCall();
            }
        "#;
        let path = Path::new("test.js");
        let analysis = ASTParser::parse_file(path, code);

        assert_eq!(analysis.nodes.len(), 3);
        assert_eq!(analysis.nodes[0].name, "User");
        assert_eq!(analysis.nodes[0].kind, "Class");
        assert_eq!(analysis.nodes[1].name, "login");
        assert_eq!(analysis.nodes[1].kind, "Method");
        assert_eq!(analysis.nodes[2].name, "register");
        assert_eq!(analysis.nodes[2].kind, "Function");

        assert_eq!(analysis.edges.len(), 3);
        assert_eq!(analysis.edges[0].from_id, "test.js");
        assert_eq!(analysis.edges[0].to_id, "test.js::User");
        assert_eq!(analysis.edges[1].from_id, "test.js::User");
        assert_eq!(analysis.edges[1].to_id, "test.js::User::login");
        assert_eq!(analysis.edges[2].from_id, "test.js");
        assert_eq!(analysis.edges[2].to_id, "test.js::register");

        assert_eq!(analysis.imports.len(), 2);
        assert_eq!(analysis.imports[0].path, "./module.js");
        assert_eq!(analysis.imports[1].path, "fs");

        assert_eq!(analysis.calls.len(), 2);
        assert_eq!(analysis.calls[0].name, "log");
        assert!(analysis.calls[0].is_method);
        assert_eq!(analysis.calls[1].name, "doJSCall");
        assert!(!analysis.calls[1].is_method);
    }

    #[test]
    fn test_ts_parsing() {
        let code = r#"
            import { service } from './service';

            interface ILogger {
                log(msg: string): void;
            }
            @logger
            class FileLogger implements ILogger {
                log(msg: string) {
                    service.info(msg);
                }
            }
        "#;
        let path = Path::new("test.ts");
        let analysis = ASTParser::parse_file(path, code);

        assert_eq!(analysis.nodes.len(), 3);
        assert_eq!(analysis.nodes[0].name, "ILogger");
        assert_eq!(analysis.nodes[0].kind, "Interface");
        assert_eq!(analysis.nodes[1].name, "FileLogger");
        assert_eq!(analysis.nodes[1].kind, "Class");
        assert_eq!(analysis.nodes[2].name, "log");
        assert_eq!(analysis.nodes[2].kind, "Method");

        assert_eq!(analysis.edges.len(), 3);
        assert_eq!(analysis.edges[0].from_id, "test.ts");
        assert_eq!(analysis.edges[0].to_id, "test.ts::ILogger");
        assert_eq!(analysis.edges[1].from_id, "test.ts");
        assert_eq!(analysis.edges[1].to_id, "test.ts::FileLogger");
        assert_eq!(analysis.edges[2].from_id, "test.ts::FileLogger");
        assert_eq!(analysis.edges[2].to_id, "test.ts::FileLogger::log");

        assert_eq!(analysis.imports.len(), 1);
        assert_eq!(analysis.imports[0].path, "./service");

        assert_eq!(analysis.calls.len(), 1);
        assert_eq!(analysis.calls[0].name, "info");
        assert!(analysis.calls[0].is_method);
    }
}
