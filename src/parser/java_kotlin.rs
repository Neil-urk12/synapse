use crate::parser::{extract_signature, LanguageParser, TraverseContext};
use crate::types::ast::{EdgeData, FileAnalysis, NodeData, RawCall, RawImport};
use tree_sitter::{Node, Parser};

pub struct JavaKotlinParser;

impl LanguageParser for JavaKotlinParser {
    fn parse(&self, content: &str, file_path: &str) -> FileAnalysis {
        let lang = if file_path.ends_with(".kt") || file_path.ends_with(".kts") {
            tree_sitter_kotlin::language()
        } else {
            tree_sitter_java::language()
        };

        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        let mut imports = Vec::new();
        let mut calls = Vec::new();

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

        if file_path.ends_with(".kt") || file_path.ends_with(".kts") {
            traverse_kotlin(tree.root_node(), &mut ctx, None);
        } else {
            traverse_java(tree.root_node(), &mut ctx, None);
        }

        FileAnalysis {
            nodes,
            edges,
            imports,
            calls,
        }
    }
}

fn traverse_java(node: Node, ctx: &mut TraverseContext, current_parent_id: Option<String>) {
    let kind = node.kind();
    let mut active_parent = current_parent_id;
    let start_point = node.start_position();
    let end_point = node.end_position();

    match kind {
        "import_declaration" => {
            let mut cursor = node.walk();
            if cursor.goto_first_child() {
                loop {
                    let child = cursor.node();
                    if child.kind() == "scoped_identifier" || child.kind() == "identifier" {
                        let path = child.utf8_text(ctx.source).unwrap_or("").to_owned();
                        if !path.is_empty() {
                            ctx.imports.push(RawImport {
                                path,
                                line: start_point.row + 1,
                            });
                        }
                        break;
                    }
                    if !cursor.goto_next_sibling() {
                        break;
                    }
                }
            }
        }
        "method_invocation" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = name_node.utf8_text(ctx.source).unwrap_or("").to_owned();
                let has_receiver = node.child_by_field_name("object").is_some();
                let is_valid =
                    !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_');
                if is_valid {
                    ctx.calls.push(RawCall {
                        name,
                        line: start_point.row + 1,
                        is_method: has_receiver,
                    });
                }
            }
        }
        "class_declaration" | "interface_declaration" => {
            let name = if let Some(name_node) = node.child_by_field_name("name") {
                name_node
                    .utf8_text(ctx.source)
                    .unwrap_or("anonymous")
                    .to_owned()
            } else {
                "anonymous".to_owned()
            };

            let kind_label = if kind == "class_declaration" {
                "Class"
            } else {
                "Interface"
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
        "method_declaration" | "constructor_declaration" => {
            let name = if let Some(name_node) = node.child_by_field_name("name") {
                name_node
                    .utf8_text(ctx.source)
                    .unwrap_or("anonymous")
                    .to_owned()
            } else {
                "anonymous".to_owned()
            };

            let kind_label = if kind == "constructor_declaration" {
                "Constructor"
            } else {
                "Method"
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
            traverse_java(cursor.node(), ctx, active_parent.clone());
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
}

fn traverse_kotlin(node: Node, ctx: &mut TraverseContext, current_parent_id: Option<String>) {
    let kind = node.kind();
    let mut active_parent = current_parent_id;
    let start_point = node.start_position();
    let end_point = node.end_position();

    match kind {
        "import_header" => {
            let mut cursor = node.walk();
            if cursor.goto_first_child() {
                loop {
                    let child = cursor.node();
                    if child.kind() == "identifier" {
                        let path = child.utf8_text(ctx.source).unwrap_or("").to_owned();
                        if !path.is_empty() {
                            ctx.imports.push(RawImport {
                                path,
                                line: start_point.row + 1,
                            });
                        }
                        break;
                    }
                    if !cursor.goto_next_sibling() {
                        break;
                    }
                }
            }
        }
        "call_expression" => {
            let mut name = String::new();
            let mut has_receiver = false;

            let mut cursor = node.walk();
            if cursor.goto_first_child() {
                let first_child = cursor.node();
                if first_child.kind() == "navigation_expression" {
                    has_receiver = true;
                    let mut inner_cursor = first_child.walk();
                    if inner_cursor.goto_first_child() {
                        loop {
                            let suffix_node = inner_cursor.node();
                            if suffix_node.kind() == "navigation_suffix" {
                                let mut suffix_cursor = suffix_node.walk();
                                if suffix_cursor.goto_first_child() {
                                    loop {
                                        let target = suffix_cursor.node();
                                        if target.kind() == "simple_identifier" {
                                            name = target
                                                .utf8_text(ctx.source)
                                                .unwrap_or("")
                                                .to_owned();
                                            break;
                                        }
                                        if !suffix_cursor.goto_next_sibling() {
                                            break;
                                        }
                                    }
                                }
                                break;
                            }
                            if !inner_cursor.goto_next_sibling() {
                                break;
                            }
                        }
                    }
                } else if first_child.kind() == "simple_identifier" {
                    name = first_child.utf8_text(ctx.source).unwrap_or("").to_owned();
                }
            }

            let is_valid =
                !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_');
            if is_valid {
                ctx.calls.push(RawCall {
                    name,
                    line: start_point.row + 1,
                    is_method: has_receiver,
                });
            }
        }
        "class_declaration" | "object_declaration" | "interface_declaration" => {
            let mut name = "anonymous".to_owned();
            let mut cursor = node.walk();
            if cursor.goto_first_child() {
                loop {
                    let child = cursor.node();
                    if child.kind() == "simple_identifier" || child.kind() == "type_identifier" {
                        name = child
                            .utf8_text(ctx.source)
                            .unwrap_or("anonymous")
                            .to_owned();
                        break;
                    }
                    if !cursor.goto_next_sibling() {
                        break;
                    }
                }
            }

            let kind_label = match kind {
                "class_declaration" | "object_declaration" => "Class",
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

            let from_id = active_parent.as_deref().unwrap_or(ctx.file_path).to_owned();
            ctx.edges.push(EdgeData {
                from_id,
                to_id: symbol_id.clone(),
                edge_type: "CONTAINS".to_owned(),
            });

            active_parent = Some(symbol_id);
        }
        "function_declaration" => {
            let mut name = "anonymous".to_owned();
            let mut cursor = node.walk();
            if cursor.goto_first_child() {
                loop {
                    let child = cursor.node();
                    if child.kind() == "simple_identifier" {
                        name = child
                            .utf8_text(ctx.source)
                            .unwrap_or("anonymous")
                            .to_owned();
                        break;
                    }
                    if !cursor.goto_next_sibling() {
                        break;
                    }
                }
            }

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
            traverse_kotlin(cursor.node(), ctx, active_parent.clone());
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
    fn test_java_parsing() {
        let code = r#"
            import java.util.List;
            public class Application {
                public Application() {}
                public void start() {
                    System.out.println("Started");
                }
            }
        "#;
        let path = Path::new("test.java");
        let FileAnalysis {
            nodes,
            edges: _,
            imports,
            calls,
        } = ASTParser::parse_file(path, code);

        assert!(!nodes.is_empty(), "Java nodes should not be empty");
        assert!(imports.iter().any(|i| i.path == "java.util.List"));

        let class_node = nodes
            .iter()
            .find(|n| n.name == "Application")
            .expect("Application class not found");
        assert_eq!(class_node.kind, "Class");

        let method_node = nodes
            .iter()
            .find(|n| n.name == "start")
            .expect("start method not found");
        assert_eq!(method_node.kind, "Method");

        assert!(calls.iter().any(|c| c.name == "println" && c.is_method));
    }

    #[test]
    fn test_kotlin_parsing() {
        let code = r#"
            import foo.bar.Baz
            class Service {
                fun execute() {
                    val x = Baz()
                    x.doSomething()
                }
            }
        "#;
        let path = Path::new("test.kt");
        let FileAnalysis {
            nodes,
            edges: _,
            imports,
            calls,
        } = ASTParser::parse_file(path, code);

        assert!(!nodes.is_empty(), "Kotlin nodes should not be empty");
        assert!(imports.iter().any(|i| i.path == "foo.bar.Baz"));

        let class_node = nodes
            .iter()
            .find(|n| n.name == "Service")
            .expect("Service class not found");
        assert_eq!(class_node.kind, "Class");

        let method_node = nodes
            .iter()
            .find(|n| n.name == "execute")
            .expect("execute method not found");
        assert_eq!(method_node.kind, "Method");

        assert!(calls.iter().any(|c| c.name == "doSomething" && c.is_method));
    }
}
