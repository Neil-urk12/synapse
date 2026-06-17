use crate::parser::{extract_signature, LanguageParser, TraverseContext};
use crate::types::ast::{EdgeData, FileAnalysis, NodeData, RawCall, RawImport};
use tree_sitter::{Node, Parser};

pub struct RubyParser;

impl LanguageParser for RubyParser {
    fn parse(&self, content: &str, file_path: &str) -> FileAnalysis {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        let mut imports = Vec::new();
        let mut calls = Vec::new();

        let mut parser = Parser::new();
        if parser.set_language(&tree_sitter_ruby::language()).is_err() {
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
        "call" => {
            if let Some(func_node) = node.child_by_field_name("method") {
                let name = func_node.utf8_text(ctx.source).unwrap_or("").to_owned();

                if func_node.kind() == "identifier" {
                    // Check if this is a require/load call
                    if name == "require" || name == "require_relative" || name == "load" {
                        // Extract path from argument_list
                        if let Some(arg_node) = node.child_by_field_name("arguments") {
                            if let Some(path_node) = arg_node.child(0) {
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
                    } else {
                        let has_receiver = node.child_by_field_name("receiver").is_some();
                        let is_valid = !name.is_empty()
                            && name.chars().all(|c| c.is_alphanumeric() || c == '_');
                        if is_valid {
                            ctx.calls.push(RawCall {
                                name,
                                line: start_point.row + 1,
                                is_method: has_receiver,
                            });
                        }
                    }
                }
            }
        }
        "method" | "singleton_method" => {
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
        "class" | "module" => {
            let name = if let Some(name_node) = node.child_by_field_name("name") {
                name_node
                    .utf8_text(ctx.source)
                    .unwrap_or("anonymous")
                    .to_owned()
            } else {
                "anonymous".to_owned()
            };

            let kind_label = if kind == "class" { "Class" } else { "Module" };

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
    fn test_ruby_parsing() {
        let code = r#"
require 'json'
require_relative './helpers'

class User
  def initialize(name)
    @name = name
  end

  def greet
    puts "Hello #{@name}"
  end
end

module Math
  def self.add(a, b)
    a + b
  end
end

def top_level_method
  Math.add(1, 2)
end
        "#;
        let path = Path::new("test.rb");
        let FileAnalysis {
            nodes,
            edges: _,
            imports,
            calls,
        } = ASTParser::parse_file(path, code);

        assert!(!nodes.is_empty(), "Ruby nodes should not be empty");
        assert!(imports.iter().any(|i| i.path == "json"));
        assert!(imports.iter().any(|i| i.path == "./helpers"));

        let class_node = nodes
            .iter()
            .find(|n| n.name == "User")
            .expect("User class not found");
        assert_eq!(class_node.kind, "Class");

        let module_node = nodes
            .iter()
            .find(|n| n.name == "Math")
            .expect("Math module not found");
        assert_eq!(module_node.kind, "Module");

        let method_node = nodes
            .iter()
            .find(|n| n.name == "initialize")
            .expect("initialize method not found");
        assert_eq!(method_node.kind, "Method");

        let func_node = nodes
            .iter()
            .find(|n| n.name == "top_level_method")
            .expect("top_level_method not found");
        assert_eq!(func_node.kind, "Method");

        let singleton_node = nodes
            .iter()
            .find(|n| n.name == "add")
            .expect("singleton method add not found");
        assert_eq!(singleton_node.kind, "Method");

        // Call-graph regression assertions: `puts` is a bare call (no receiver),
        // `Math.add` has a receiver so it should be flagged is_method=true.
        assert!(
            calls.iter().any(|c| c.name == "puts" && !c.is_method),
            "bare call puts should be captured with is_method=false, got: {:?}",
            calls
        );
        assert!(
            calls.iter().any(|c| c.name == "add" && c.is_method),
            "Math.add should be captured with is_method=true (has receiver), got: {:?}",
            calls
        );
    }
}
