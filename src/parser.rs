use std::path::Path;
use tree_sitter::{Parser, Node, Language};

#[derive(Debug, Clone, PartialEq)]
pub struct NodeData {
    pub id: String,
    pub name: String,
    pub kind: String, // "Function", "Class", "Struct", "Method", "Interface", "Implementation", etc.
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EdgeData {
    pub from_id: String,
    pub to_id: String,
    pub edge_type: String, // "CONTAINS"
    pub line: usize,
}

pub struct ASTParser;

impl ASTParser {
    fn get_language(extension: &str) -> Option<Language> {
        match extension {
            "rs" => Some(tree_sitter_rust::language()),
            "js" | "jsx" => Some(tree_sitter_javascript::language()),
            "ts" => Some(tree_sitter_typescript::language_typescript()),
            "tsx" => Some(tree_sitter_typescript::language_tsx()),
            _ => None,
        }
    }

    pub fn parse_file(path: &Path, content: &str) -> (Vec<NodeData>, Vec<EdgeData>) {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();

        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let language = match Self::get_language(ext) {
            Some(lang) => lang,
            None => return (nodes, edges),
        };

        let mut parser = Parser::new();
        if parser.set_language(&language).is_err() {
            return (nodes, edges);
        }

        let tree = match parser.parse(content, None) {
            Some(t) => t,
            None => return (nodes, edges),
        };

        let source_bytes = content.as_bytes();
        let file_path_str = path.to_string_lossy().to_string();

        Self::traverse(
            tree.root_node(),
            source_bytes,
            &file_path_str,
            &mut nodes,
            &mut edges,
            None,
        );

        (nodes, edges)
    }

    fn traverse(
        node: Node,
        source: &[u8],
        file_path: &str,
        nodes: &mut Vec<NodeData>,
        edges: &mut Vec<EdgeData>,
        current_parent_id: Option<String>,
    ) {
        let kind = node.kind();
        let mut active_parent = current_parent_id.clone();

        match kind {
            // Rust Declarations
            "function_item" | "struct_item" | "enum_item" | "trait_item" | "impl_item" |
            // JavaScript & TypeScript Declarations
            "function_declaration" | "class_declaration" | "method_definition" | "interface_declaration" => {
                let name = if let Some(name_node) = node.child_by_field_name("name") {
                    name_node.utf8_text(source).unwrap_or("anonymous").to_string()
                } else if kind == "impl_item" {
                    let type_name = if let Some(type_node) = node.child_by_field_name("type") {
                        type_node.utf8_text(source).unwrap_or("Type")
                    } else {
                        "Type"
                    };
                    if let Some(trait_node) = node.child_by_field_name("trait") {
                        let trait_name = trait_node.utf8_text(source).unwrap_or("Trait");
                        format!("impl {} for {}", trait_name, type_name)
                    } else {
                        format!("impl {}", type_name)
                    }
                } else {
                    "anonymous".to_string()
                };

                let kind_label = match kind {
                    "function_item" | "function_declaration" => "Function",
                    "struct_item" => "Struct",
                    "enum_item" => "Enum",
                    "trait_item" | "interface_declaration" => "Interface",
                    "impl_item" => "Implementation",
                    "class_declaration" => "Class",
                    "method_definition" => "Method",
                    _ => "Symbol",
                };

                let start_point = node.start_position();
                let end_point = node.end_position();

                let symbol_id = if let Some(ref parent) = current_parent_id {
                    format!("{}::{}", parent, name)
                } else {
                    format!("{}::{}", file_path, name)
                };

                let mut start_byte = node.start_byte();
                
                // Skip leading decorators for the signature definition
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
                
                // Trim leading whitespace/newlines from the start byte
                while start_byte < source.len() && source[start_byte].is_ascii_whitespace() {
                    start_byte += 1;
                }

                let mut end_line_byte = start_byte;
                while end_line_byte < source.len() && source[end_line_byte] != b'\n' {
                    end_line_byte += 1;
                }
                let signature = String::from_utf8_lossy(&source[start_byte..end_line_byte])
                    .trim()
                    .to_string();

                nodes.push(NodeData {
                    id: symbol_id.clone(),
                    name,
                    kind: kind_label.to_string(),
                    start_line: start_point.row + 1,
                    start_col: start_point.column + 1,
                    end_line: end_point.row + 1,
                    signature,
                });

                if current_parent_id.is_none() {
                    edges.push(EdgeData {
                        from_id: file_path.to_string(),
                        to_id: symbol_id.clone(),
                        edge_type: "CONTAINS".to_string(),
                        line: start_point.row + 1,
                    });
                } else if let Some(ref parent) = current_parent_id {
                    edges.push(EdgeData {
                        from_id: parent.clone(),
                        to_id: symbol_id.clone(),
                        edge_type: "CONTAINS".to_string(),
                        line: start_point.row + 1,
                    });
                }

                active_parent = Some(symbol_id);
            }
            _ => {}
        }

        let mut cursor = node.walk();
        if cursor.goto_first_child() {
            loop {
                Self::traverse(
                    cursor.node(),
                    source,
                    file_path,
                    nodes,
                    edges,
                    active_parent.clone(),
                );
                if !cursor.goto_next_sibling() {
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_rust_parsing() {
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
        let (nodes, edges) = ASTParser::parse_file(path, code);

        assert!(!nodes.is_empty(), "Nodes should not be empty");
        
        // Nodes check
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

        // Verify some specific nodes
        assert_eq!(nodes[0].id, "test.rs::MyEnum");
        assert_eq!(nodes[1].id, "test.rs::MyTrait");
        assert_eq!(nodes[2].id, "test.rs::MyTrait::trait_func");
        assert_eq!(nodes[3].id, "test.rs::MyStruct");
        assert_eq!(nodes[4].id, "test.rs::impl MyStruct");
        assert_eq!(nodes[5].id, "test.rs::impl MyStruct::run");
        assert_eq!(nodes[6].id, "test.rs::impl MyTrait for MyStruct");
        assert_eq!(nodes[7].id, "test.rs::impl MyTrait for MyStruct::trait_func");

        // Verify edges
        assert_eq!(edges.len(), 8);
        assert_eq!(edges[0].edge_type, "CONTAINS");
        assert_eq!(edges[0].from_id, "test.rs");
        assert_eq!(edges[0].to_id, "test.rs::MyEnum");
        
        assert_eq!(edges[1].from_id, "test.rs");
        assert_eq!(edges[1].to_id, "test.rs::MyTrait");

        assert_eq!(edges[2].from_id, "test.rs::MyTrait");
        assert_eq!(edges[2].to_id, "test.rs::MyTrait::trait_func");

        assert_eq!(edges[3].from_id, "test.rs");
        assert_eq!(edges[3].to_id, "test.rs::MyStruct");

        assert_eq!(edges[4].from_id, "test.rs");
        assert_eq!(edges[4].to_id, "test.rs::impl MyStruct");

        assert_eq!(edges[5].from_id, "test.rs::impl MyStruct");
        assert_eq!(edges[5].to_id, "test.rs::impl MyStruct::run");

        assert_eq!(edges[6].from_id, "test.rs");
        assert_eq!(edges[6].to_id, "test.rs::impl MyTrait for MyStruct");

        assert_eq!(edges[7].from_id, "test.rs::impl MyTrait for MyStruct");
        assert_eq!(edges[7].to_id, "test.rs::impl MyTrait for MyStruct::trait_func");
    }

    #[test]
    fn test_js_parsing() {
        let code = r#"
            class User {
                login() {}
            }
            function register() {}
        "#;
        let path = Path::new("test.js");
        let (nodes, edges) = ASTParser::parse_file(path, code);

        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[0].name, "User");
        assert_eq!(nodes[0].kind, "Class");
        assert_eq!(nodes[1].name, "login");
        assert_eq!(nodes[1].kind, "Method");
        assert_eq!(nodes[2].name, "register");
        assert_eq!(nodes[2].kind, "Function");

        assert_eq!(edges.len(), 3);
        assert_eq!(edges[0].from_id, "test.js");
        assert_eq!(edges[0].to_id, "test.js::User");
        assert_eq!(edges[1].from_id, "test.js::User");
        assert_eq!(edges[1].to_id, "test.js::User::login");
        assert_eq!(edges[2].from_id, "test.js");
        assert_eq!(edges[2].to_id, "test.js::register");
    }

    #[test]
    fn test_ts_parsing() {
        let code = r#"
            interface ILogger {
                log(msg: string): void;
            }
            @logger
            class FileLogger implements ILogger {
                log(msg: string) {}
            }
        "#;
        let path = Path::new("test.ts");
        let (nodes, edges) = ASTParser::parse_file(path, code);

        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[0].name, "ILogger");
        assert_eq!(nodes[0].kind, "Interface");
        assert_eq!(nodes[1].name, "FileLogger");
        assert_eq!(nodes[1].kind, "Class");
        assert_eq!(nodes[2].name, "log");
        assert_eq!(nodes[2].kind, "Method");

        assert_eq!(edges.len(), 3);
        assert_eq!(edges[0].from_id, "test.ts");
        assert_eq!(edges[0].to_id, "test.ts::ILogger");
        assert_eq!(edges[1].from_id, "test.ts");
        assert_eq!(edges[1].to_id, "test.ts::FileLogger");
        assert_eq!(edges[2].from_id, "test.ts::FileLogger");
        assert_eq!(edges[2].to_id, "test.ts::FileLogger::log");
    }
}
