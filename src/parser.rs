use crate::types::ast::{EdgeData, FileAnalysis, NodeData, RawCall, RawImport};
use std::path::Path;
use tree_sitter::{Language, Node, Parser};

struct TraverseContext<'a> {
    source: &'a [u8],
    file_path: &'a str,
    nodes: &'a mut Vec<NodeData>,
    edges: &'a mut Vec<EdgeData>,
    imports: &'a mut Vec<RawImport>,
    calls: &'a mut Vec<RawCall>,
}

pub struct ASTParser;

impl ASTParser {
    fn get_language(extension: &str) -> Option<Language> {
        match extension {
            "rs" => Some(tree_sitter_rust::language()),
            "js" | "jsx" => Some(tree_sitter_javascript::language()),
            "ts" => Some(tree_sitter_typescript::language_typescript()),
            "tsx" => Some(tree_sitter_typescript::language_tsx()),
            "go" => Some(tree_sitter_go::language()),
            "py" => Some(tree_sitter_python::language()),
            "c" => Some(tree_sitter_c::language()),
            "cpp" | "cc" | "cxx" | "h" | "hpp" => Some(tree_sitter_cpp::language()),
            "java" => Some(tree_sitter_java::language()),
            "kt" | "kts" => Some(tree_sitter_kotlin::language()),
            _ => None,
        }
    }

    pub fn parse_file(path: &Path, content: &str) -> FileAnalysis {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        let mut imports = Vec::new();
        let mut calls = Vec::new();

        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let language = match Self::get_language(ext) {
            Some(lang) => lang,
            None => {
                return FileAnalysis {
                    nodes,
                    edges,
                    imports,
                    calls,
                }
            }
        };

        let mut parser = Parser::new();
        if parser.set_language(&language).is_err() {
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
        let file_path_str = path.to_string_lossy().to_string();

        let mut ctx = TraverseContext {
            source: source_bytes,
            file_path: &file_path_str,
            nodes: &mut nodes,
            edges: &mut edges,
            imports: &mut imports,
            calls: &mut calls,
        };

        Self::traverse(tree.root_node(), &mut ctx, None);

        FileAnalysis {
            nodes,
            edges,
            imports,
            calls,
        }
    }

    fn traverse(node: Node, ctx: &mut TraverseContext, current_parent_id: Option<String>) {
        if ctx.file_path.ends_with(".go") {
            Self::traverse_go(node, ctx, current_parent_id);
            return;
        }
        if ctx.file_path.ends_with(".py") {
            Self::traverse_python(node, ctx, current_parent_id);
            return;
        }
        if ctx.file_path.ends_with(".cpp")
            || ctx.file_path.ends_with(".cc")
            || ctx.file_path.ends_with(".cxx")
            || ctx.file_path.ends_with(".c")
            || ctx.file_path.ends_with(".h")
            || ctx.file_path.ends_with(".hpp")
        {
            Self::traverse_cpp(node, ctx, current_parent_id);
            return;
        }
        if ctx.file_path.ends_with(".java") {
            Self::traverse_java(node, ctx, current_parent_id);
            return;
        }
        if ctx.file_path.ends_with(".kt") || ctx.file_path.ends_with(".kts") {
            Self::traverse_kotlin(node, ctx, current_parent_id);
            return;
        }
        let kind = node.kind();
        let mut active_parent = current_parent_id.clone();
        let start_point = node.start_position();
        let is_js_ts = ctx.file_path.ends_with(".js")
            || ctx.file_path.ends_with(".jsx")
            || ctx.file_path.ends_with(".ts")
            || ctx.file_path.ends_with(".tsx");

        match kind {
            // -- Rust Imports & Calls --
            "use_declaration" => {
                if !is_js_ts {
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
            }
            "method_call_expression" => {
                if !is_js_ts {
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
            }

            // -- JS/TS Imports & Calls --
            "import_statement" | "export_statement" => {
                if is_js_ts {
                    if let Some(source_node) = node.child_by_field_name("source") {
                        let path = source_node
                            .utf8_text(ctx.source)
                            .unwrap_or("")
                            .trim_matches(|c| c == '\'' || c == '"')
                            .to_string();
                        if !path.is_empty() {
                            ctx.imports.push(RawImport {
                                path,
                                line: start_point.row + 1,
                            });
                        }
                    }
                }
            }
            "call_expression" => {
                if let Some(func_node) = node.child_by_field_name("function") {
                    if is_js_ts {
                        let name = func_node.utf8_text(ctx.source).unwrap_or("").to_string();
                        if name == "require" {
                            // Extract first argument string
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
                                                        .to_string();
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
                                    prop_node.utf8_text(ctx.source).unwrap_or("").to_string();
                                let is_valid = !method_name.is_empty()
                                    && method_name.chars().all(|c| {
                                        c.is_alphanumeric() || c == '_' || c == '$' || c == ':'
                                    });
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
                                && name.chars().all(|c| {
                                    c.is_alphanumeric() || c == '_' || c == '$' || c == ':'
                                });
                            if is_valid {
                                ctx.calls.push(RawCall {
                                    name,
                                    line: start_point.row + 1,
                                    is_method: false,
                                });
                            }
                        }
                    } else {
                        // Rust
                        let mut name = func_node.utf8_text(ctx.source).unwrap_or("").to_string();
                        if func_node.kind() == "field_expression" {
                            if let Some(field_node) = func_node.child_by_field_name("field") {
                                let method_name =
                                    field_node.utf8_text(ctx.source).unwrap_or("").to_string();
                                let is_valid = !method_name.is_empty()
                                    && method_name.chars().all(|c| {
                                        c.is_alphanumeric() || c == '_' || c == '$' || c == ':'
                                    });
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
                                && name.chars().all(|c| {
                                    c.is_alphanumeric() || c == '_' || c == '$' || c == ':'
                                });
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
            }

            // Rust & JS/TS Declarations (same as before)
            "function_item"
            | "struct_item"
            | "enum_item"
            | "trait_item"
            | "impl_item"
            | "function_declaration"
            | "class_declaration"
            | "method_definition"
            | "interface_declaration" => {
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
                    "function_item" | "function_declaration" => "Function",
                    "struct_item" => "Struct",
                    "enum_item" => "Enum",
                    "trait_item" | "interface_declaration" => "Interface",
                    "impl_item" => "Implementation",
                    "class_declaration" => "Class",
                    "method_definition" => "Method",
                    _ => "Symbol",
                };

                let end_point = node.end_position();

                let symbol_id = if let Some(ref parent) = current_parent_id {
                    format!("{}::{}", parent, name)
                } else {
                    format!("{}::{}", ctx.file_path, name)
                };

                let mut start_byte = node.start_byte();
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
                while start_byte < ctx.source.len() && ctx.source[start_byte].is_ascii_whitespace()
                {
                    start_byte += 1;
                }

                let mut end_line_byte = start_byte;
                while end_line_byte < ctx.source.len() && ctx.source[end_line_byte] != b'\n' {
                    end_line_byte += 1;
                }
                let signature = String::from_utf8_lossy(&ctx.source[start_byte..end_line_byte])
                    .trim()
                    .to_string();

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
                Self::traverse(cursor.node(), ctx, active_parent.clone());
                if !cursor.goto_next_sibling() {
                    break;
                }
            }
        }
    }

    fn extract_signature(node: Node, source: &[u8]) -> String {
        let mut start_byte = node.start_byte();
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
        while start_byte < source.len() && source[start_byte].is_ascii_whitespace() {
            start_byte += 1;
        }

        let mut end_line_byte = start_byte;
        while end_line_byte < source.len() && source[end_line_byte] != b'\n' {
            end_line_byte += 1;
        }
        String::from_utf8_lossy(&source[start_byte..end_line_byte])
            .trim()
            .to_string()
    }

    fn traverse_go(node: Node, ctx: &mut TraverseContext, current_parent_id: Option<String>) {
        let kind = node.kind();
        let mut active_parent = current_parent_id.clone();
        let start_point = node.start_position();
        let end_point = node.end_position();

        match kind {
            "import_spec" => {
                if let Some(path_node) = node.child_by_field_name("path") {
                    let path = path_node
                        .utf8_text(ctx.source)
                        .unwrap_or("")
                        .trim_matches(|c| c == '\'' || c == '"')
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
                    let name = func_node.utf8_text(ctx.source).unwrap_or("").to_string();
                    if func_node.kind() == "selector_expression" {
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
                    } else {
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
            "function_declaration" | "method_declaration" => {
                let name = if let Some(name_node) = node.child_by_field_name("name") {
                    name_node
                        .utf8_text(ctx.source)
                        .unwrap_or("anonymous")
                        .to_string()
                } else {
                    "anonymous".to_string()
                };

                let kind_label = if kind == "method_declaration" {
                    "Method"
                } else {
                    "Function"
                };
                let mut go_parent = current_parent_id.clone();
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
                                        Some(type_text.trim_start_matches('*').trim().to_string());
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

                let signature = Self::extract_signature(node, ctx.source);

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
                                    .to_string();
                                if let Some(type_node) = child.child_by_field_name("type") {
                                    let kind_label = match type_node.kind() {
                                        "struct_type" => "Struct",
                                        "interface_type" => "Interface",
                                        _ => "",
                                    };
                                    if !kind_label.is_empty() {
                                        let symbol_id = if let Some(ref parent) = current_parent_id
                                        {
                                            format!("{}::{}", parent, name)
                                        } else {
                                            format!("{}::{}", ctx.file_path, name)
                                        };

                                        let signature = Self::extract_signature(node, ctx.source);

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
                Self::traverse_go(cursor.node(), ctx, active_parent.clone());
                if !cursor.goto_next_sibling() {
                    break;
                }
            }
        }
    }

    fn traverse_python(node: Node, ctx: &mut TraverseContext, current_parent_id: Option<String>) {
        let kind = node.kind();
        let mut active_parent = current_parent_id.clone();
        let start_point = node.start_position();
        let end_point = node.end_position();

        match kind {
            "import_statement" => {
                let mut cursor = node.walk();
                if cursor.goto_first_child() {
                    loop {
                        let child = cursor.node();
                        if child.kind() == "dotted_name" {
                            let path = child.utf8_text(ctx.source).unwrap_or("").to_string();
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
                                module_path = child.utf8_text(ctx.source).unwrap_or("").to_string();
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
                            let mut name_text =
                                child.utf8_text(ctx.source).unwrap_or("").to_string();
                            if child.kind() == "aliased_import" {
                                if let Some(name_node) = child.child_by_field_name("name") {
                                    name_text =
                                        name_node.utf8_text(ctx.source).unwrap_or("").to_string();
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
                    let name = func_node.utf8_text(ctx.source).unwrap_or("").to_string();
                    if func_node.kind() == "attribute" {
                        if let Some(attribute_node) = func_node.child_by_field_name("attribute") {
                            let method_name = attribute_node
                                .utf8_text(ctx.source)
                                .unwrap_or("")
                                .to_string();
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
            "class_definition" => {
                let name = if let Some(name_node) = node.child_by_field_name("name") {
                    name_node
                        .utf8_text(ctx.source)
                        .unwrap_or("anonymous")
                        .to_string()
                } else {
                    "anonymous".to_string()
                };

                let symbol_id = if let Some(ref parent) = current_parent_id {
                    format!("{}::{}", parent, name)
                } else {
                    format!("{}::{}", ctx.file_path, name)
                };

                let signature = Self::extract_signature(node, ctx.source);

                ctx.nodes.push(NodeData {
                    id: symbol_id.clone(),
                    name,
                    kind: "Class".to_string(),
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
                let name = if let Some(name_node) = node.child_by_field_name("name") {
                    name_node
                        .utf8_text(ctx.source)
                        .unwrap_or("anonymous")
                        .to_string()
                } else {
                    "anonymous".to_string()
                };

                let mut is_method = false;
                if let Some(ref parent) = current_parent_id {
                    if let Some(parent_node) = ctx.nodes.iter().find(|n| &n.id == parent) {
                        if parent_node.kind == "Class" {
                            is_method = true;
                        }
                    }
                }

                let kind_label = if is_method { "Method" } else { "Function" };

                let symbol_id = if let Some(ref parent) = current_parent_id {
                    format!("{}::{}", parent, name)
                } else {
                    format!("{}::{}", ctx.file_path, name)
                };

                let signature = Self::extract_signature(node, ctx.source);

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
                Self::traverse_python(cursor.node(), ctx, active_parent.clone());
                if !cursor.goto_next_sibling() {
                    break;
                }
            }
        }
    }

    fn traverse_cpp(node: Node, ctx: &mut TraverseContext, current_parent_id: Option<String>) {
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

                let signature = Self::extract_signature(node, ctx.source);

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

                let signature = Self::extract_signature(node, ctx.source);

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
                Self::traverse_cpp(cursor.node(), ctx, active_parent.clone());
                if !cursor.goto_next_sibling() {
                    break;
                }
            }
        }
    }

    fn traverse_java(node: Node, ctx: &mut TraverseContext, current_parent_id: Option<String>) {
        let kind = node.kind();
        let mut active_parent = current_parent_id.clone();
        let start_point = node.start_position();
        let end_point = node.end_position();

        match kind {
            "import_declaration" => {
                let mut cursor = node.walk();
                if cursor.goto_first_child() {
                    loop {
                        let child = cursor.node();
                        if child.kind() == "scoped_identifier" || child.kind() == "identifier" {
                            let path = child.utf8_text(ctx.source).unwrap_or("").to_string();
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
                    let name = name_node.utf8_text(ctx.source).unwrap_or("").to_string();
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
                        .to_string()
                } else {
                    "anonymous".to_string()
                };

                let kind_label = if kind == "class_declaration" {
                    "Class"
                } else {
                    "Interface"
                };

                let symbol_id = if let Some(ref parent) = current_parent_id {
                    format!("{}::{}", parent, name)
                } else {
                    format!("{}::{}", ctx.file_path, name)
                };

                let signature = Self::extract_signature(node, ctx.source);

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
            "method_declaration" | "constructor_declaration" => {
                let name = if let Some(name_node) = node.child_by_field_name("name") {
                    name_node
                        .utf8_text(ctx.source)
                        .unwrap_or("anonymous")
                        .to_string()
                } else {
                    "anonymous".to_string()
                };

                let kind_label = if kind == "constructor_declaration" {
                    "Constructor"
                } else {
                    "Method"
                };

                let symbol_id = if let Some(ref parent) = current_parent_id {
                    format!("{}::{}", parent, name)
                } else {
                    format!("{}::{}", ctx.file_path, name)
                };

                let signature = Self::extract_signature(node, ctx.source);

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
                Self::traverse_java(cursor.node(), ctx, active_parent.clone());
                if !cursor.goto_next_sibling() {
                    break;
                }
            }
        }
    }

    fn traverse_kotlin(node: Node, ctx: &mut TraverseContext, current_parent_id: Option<String>) {
        let kind = node.kind();
        let mut active_parent = current_parent_id.clone();
        let start_point = node.start_position();
        let end_point = node.end_position();

        match kind {
            "import_header" => {
                let mut cursor = node.walk();
                if cursor.goto_first_child() {
                    loop {
                        let child = cursor.node();
                        if child.kind() == "identifier" {
                            let path = child.utf8_text(ctx.source).unwrap_or("").to_string();
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
                                                    .to_string();
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
                        name = first_child.utf8_text(ctx.source).unwrap_or("").to_string();
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
                let mut name = "anonymous".to_string();
                let mut cursor = node.walk();
                if cursor.goto_first_child() {
                    loop {
                        let child = cursor.node();
                        if child.kind() == "simple_identifier" || child.kind() == "type_identifier"
                        {
                            name = child
                                .utf8_text(ctx.source)
                                .unwrap_or("anonymous")
                                .to_string();
                            break;
                        }
                        if !cursor.goto_next_sibling() {
                            break;
                        }
                    }
                }

                let kind_label = match kind {
                    "class_declaration" => "Class",
                    "object_declaration" => "Class",
                    "interface_declaration" => "Interface",
                    _ => "Symbol",
                };

                let symbol_id = if let Some(ref parent) = current_parent_id {
                    format!("{}::{}", parent, name)
                } else {
                    format!("{}::{}", ctx.file_path, name)
                };

                let signature = Self::extract_signature(node, ctx.source);

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
            "function_declaration" => {
                let mut name = "anonymous".to_string();
                let mut cursor = node.walk();
                if cursor.goto_first_child() {
                    loop {
                        let child = cursor.node();
                        if child.kind() == "simple_identifier" {
                            name = child
                                .utf8_text(ctx.source)
                                .unwrap_or("anonymous")
                                .to_string();
                            break;
                        }
                        if !cursor.goto_next_sibling() {
                            break;
                        }
                    }
                }

                let mut is_method = false;
                if let Some(ref parent) = current_parent_id {
                    if let Some(parent_node) = ctx.nodes.iter().find(|n| &n.id == parent) {
                        if parent_node.kind == "Class" {
                            is_method = true;
                        }
                    }
                }
                let kind_label = if is_method { "Method" } else { "Function" };

                let symbol_id = if let Some(ref parent) = current_parent_id {
                    format!("{}::{}", parent, name)
                } else {
                    format!("{}::{}", ctx.file_path, name)
                };

                let signature = Self::extract_signature(node, ctx.source);

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
                Self::traverse_kotlin(cursor.node(), ctx, active_parent.clone());
                if !cursor.goto_next_sibling() {
                    break;
                }
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
    use std::path::Path;

    /// Look up a node by its stable composite ID. Panics with a descriptive message if not found.
    fn node_by_id<'a>(nodes: &'a [NodeData], id: &str) -> &'a NodeData {
        nodes.iter().find(|n| n.id == id).unwrap_or_else(|| {
            panic!(
                "node with id '{id}' not found in: {:#?}",
                nodes.iter().map(|n| &n.id).collect::<Vec<_>>()
            )
        })
    }

    /// Look up an edge by its stable from/to pair. Panics with a descriptive message if not found.
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

        // Verify specific nodes by stable ID (order-independent)
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

        // Verify edges by stable endpoints (order-independent)
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
