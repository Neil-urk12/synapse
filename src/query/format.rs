use crate::types::query::{CallerInfo, ContextPayload, FileInfo, SymbolInfo};
use std::io::Write;

pub fn format_ascii_table(headers: &[String], rows: &[Vec<String>]) -> String {
    if headers.is_empty() {
        return String::new();
    }
    let mut col_widths = vec![0; headers.len()];
    for (i, h) in headers.iter().enumerate() {
        col_widths[i] = h.len();
    }
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < col_widths.len() && cell.len() > col_widths[i] {
                col_widths[i] = cell.len();
            }
        }
    }
    let mut separator = String::new();
    for w in &col_widths {
        separator.push('+');
        separator.push_str(&"-".repeat(*w + 2));
    }
    separator.push_str("+\n");

    let mut out = String::new();
    out.push_str(&separator);
    out.push('|');
    for (i, h) in headers.iter().enumerate() {
        out.push_str(&format!(" {:<width$} |", h, width = col_widths[i]));
    }
    out.push('\n');
    out.push_str(&separator);

    for row in rows {
        out.push('|');
        for (i, cell) in row.iter().take(headers.len()).enumerate() {
            out.push_str(&format!(" {:<width$} |", cell, width = col_widths[i]));
        }
        if row.len() < headers.len() {
            for &width in col_widths.iter().take(headers.len()).skip(row.len()) {
                out.push_str(&format!(" {:<width$} |", "", width = width));
            }
        }
        out.push('\n');
    }
    out.push_str(separator.trim_end());
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryFormat {
    Json,
    Markdown,
    Table,
}

impl QueryFormat {
    /// Parse the CLI's format string for call-graph and dependencies.
    /// Accepts "json", "markdown", "table".
    pub fn parse(s: &str) -> Result<Self, Box<dyn std::error::Error>> {
        match s {
            "table" => Ok(Self::Table),
            "markdown" => Ok(Self::Markdown),
            "json" => Ok(Self::Json),
            other => Err(format!(
                "Invalid format '{}'. Supported: table, markdown, json",
                other
            )
            .into()),
        }
    }

    /// Parse the CLI's format string for `run_context`. Accepts "json" and "markdown" only.
    /// Preserves the pre-unification behavior where context didn't support table output.
    pub fn parse_for_context(s: &str) -> Result<Self, Box<dyn std::error::Error>> {
        match s {
            "markdown" => Ok(Self::Markdown),
            "json" => Ok(Self::Json),
            other => Err(format!(
                "Invalid output format '{}'. Supported formats are: markdown, json",
                other
            )
            .into()),
        }
    }

    /// Render a call-graph result (callers or callees of a target symbol).
    /// The caller provides the heading and JSON key — the format module is format-only,
    /// it doesn't know what "Callers" or "Callees" means semantically.
    pub fn render_caller_callee(
        &self,
        target: &SymbolInfo,
        edges: &[CallerInfo],
        heading: &str,
        json_key: &str,
        writer: &mut dyn Write,
    ) -> Result<(), Box<dyn std::error::Error>> {
        match self {
            QueryFormat::Json => {
                let json_val = serde_json::json!({
                    "target": target,
                    json_key: edges
                });
                writeln!(writer, "{}", serde_json::to_string_pretty(&json_val)?)?;
            }
            QueryFormat::Markdown => {
                writeln!(writer, "# {}", heading)?;
                if edges.is_empty() {
                    writeln!(writer, "\nNo results found.")?;
                } else {
                    for c in edges {
                        writeln!(
                            writer,
                            "* `{}` (kind: {}, line: {})",
                            c.id, c.kind, c.call_site_line
                        )?;
                    }
                }
            }
            QueryFormat::Table => {
                let headers = vec![
                    "Symbol ID".to_string(),
                    "Kind".to_string(),
                    "Signature".to_string(),
                    "Line".to_string(),
                ];
                let rows: Vec<Vec<String>> = edges
                    .iter()
                    .map(|c| {
                        vec![
                            c.id.clone(),
                            c.kind.clone(),
                            c.signature.clone(),
                            c.call_site_line.to_string(),
                        ]
                    })
                    .collect();
                writeln!(writer, "{}", format_ascii_table(&headers, &rows))?;
            }
        }
        Ok(())
    }

    /// Render a dependencies result (imports and imported-by of a target file).
    pub fn render_dependencies(
        &self,
        target: &FileInfo,
        imports: &[String],
        imported_by: &[String],
        writer: &mut dyn Write,
    ) -> Result<(), Box<dyn std::error::Error>> {
        match self {
            QueryFormat::Json => {
                let json_val = serde_json::json!({
                    "target": target,
                    "imports": imports,
                    "imported_by": imported_by
                });
                writeln!(writer, "{}", serde_json::to_string_pretty(&json_val)?)?;
            }
            QueryFormat::Markdown => {
                writeln!(writer, "# Dependencies of `{}`", target.path)?;
                writeln!(writer, "\n## Imports")?;
                if imports.is_empty() {
                    writeln!(writer, "No imports.")?;
                } else {
                    for imp in imports {
                        writeln!(writer, "* `{}`", imp)?;
                    }
                }
                writeln!(writer, "\n## Imported By")?;
                if imported_by.is_empty() {
                    writeln!(writer, "No files import this file.")?;
                } else {
                    for imp_by in imported_by {
                        writeln!(writer, "* `{}`", imp_by)?;
                    }
                }
            }
            QueryFormat::Table => {
                let headers = vec!["File Path".to_string(), "Direction".to_string()];
                let mut rows = Vec::new();
                for imp in imports {
                    rows.push(vec![imp.clone(), "Imports".to_string()]);
                }
                for imp_by in imported_by {
                    rows.push(vec![imp_by.clone(), "Imported By".to_string()]);
                }
                writeln!(writer, "{}", format_ascii_table(&headers, &rows))?;
            }
        }
        Ok(())
    }

    /// Render a context payload. Callers must use `Json` or `Markdown` —
    /// the CLI boundary parses with `parse_for_context`, which rejects `Table`.
    /// `Table` is kept as an unreachable arm so the match stays exhaustive.
    pub fn render_context(
        &self,
        payload: &ContextPayload,
        file_path_to_read: Option<&str>,
        writer: &mut dyn Write,
    ) -> Result<(), Box<dyn std::error::Error>> {
        match self {
            QueryFormat::Json => {
                let json_str = serde_json::to_string_pretty(payload)?;
                writeln!(writer, "{}", json_str)?;
            }
            QueryFormat::Markdown => {
                if let Some(ref sym) = payload.symbol {
                    writeln!(writer, "# Context: {} ({})", sym.id, sym.kind)?;
                    writeln!(writer, "\n## Signature\n`{}`", sym.signature)?;
                    if let Some(ref p) = file_path_to_read {
                        writeln!(
                            writer,
                            "\n## Source Code ({}:{}-{})",
                            p, sym.start_line, sym.end_line
                        )?;
                        let syntax = syntax_for_path(p);
                        writeln!(writer, "```{}\n{}\n```", syntax, payload.source_code)?;
                    }
                } else if let Some(ref fl) = payload.file {
                    writeln!(writer, "# Context: {} ({})", fl.path, fl.language)?;
                    writeln!(writer, "\n## Source Code ({})", fl.path)?;
                    let syntax = syntax_for_path(&fl.path);
                    writeln!(writer, "```{}\n{}\n```", syntax, payload.source_code)?;
                }

                if !payload.callers.is_empty() || !payload.callees.is_empty() {
                    writeln!(writer, "\n## Call Graph")?;
                    if !payload.callers.is_empty() {
                        writeln!(writer, "### Callers")?;
                        for caller in &payload.callers {
                            writeln!(
                                writer,
                                "* `{}` (kind: {}, line: {})",
                                caller.id, caller.kind, caller.call_site_line
                            )?;
                        }
                    }
                    if !payload.callees.is_empty() {
                        writeln!(writer, "### Callees")?;
                        for callee in &payload.callees {
                            writeln!(
                                writer,
                                "* `{}` (kind: {}, line: {})",
                                callee.id, callee.kind, callee.call_site_line
                            )?;
                        }
                    }
                }

                if !payload.imports.is_empty() || !payload.imported_by.is_empty() {
                    let path_ctx = file_path_to_read.unwrap_or("");
                    writeln!(writer, "\n## File Dependencies ({})", path_ctx)?;
                    if !payload.imports.is_empty() {
                        writeln!(writer, "### Imports")?;
                        for imp in &payload.imports {
                            writeln!(writer, "* `{}`", imp)?;
                        }
                    }
                    if !payload.imported_by.is_empty() {
                        writeln!(writer, "### Imported By")?;
                        for imp_by in &payload.imported_by {
                            writeln!(writer, "* `{}`", imp_by)?;
                        }
                    }
                }

                if !payload.contained_symbols.is_empty() {
                    writeln!(writer, "\n## Contained Symbols")?;
                    for sym in &payload.contained_symbols {
                        writeln!(writer, "* `{}` (kind: {})", sym.id, sym.kind)?;
                    }
                }
            }
            QueryFormat::Table => {
                // `Table` is reachable from `QueryFormat::parse` (the
                // `query` and `dependencies` subcommands use it) but not
                // from `parse_for_context` (the `context` subcommand).
                // Returning an error makes the invariant an enforced API
                // contract rather than a panic at runtime.
                return Err("render_context does not support the 'table' format; use parse_for_context instead".into());
            }
        }
        Ok(())
    }
}

fn syntax_for_path(path: &str) -> &'static str {
    if path.ends_with(".rs") {
        "rust"
    } else if path.ends_with(".js") || path.ends_with(".jsx") {
        "javascript"
    } else if path.ends_with(".ts") || path.ends_with(".tsx") {
        "typescript"
    } else {
        "text"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::query::{FileInfo, SymbolInfo};

    #[test]
    fn test_format_table_rendering() {
        let headers = vec!["ID".to_string(), "NAME".to_string(), "VAL".to_string()];
        let rows = vec![
            vec!["1".to_string(), "Alice".to_string(), "10.5".to_string()],
            vec!["20".to_string(), "Bob".to_string(), "true".to_string()],
        ];
        let formatted = format_ascii_table(&headers, &rows);
        let expected = "+----+-------+------+\n\
                        | ID | NAME  | VAL  |\n\
                        +----+-------+------+\n\
                        | 1  | Alice | 10.5 |\n\
                        | 20 | Bob   | true |\n\
                        +----+-------+------+";
        assert_eq!(formatted.trim(), expected);
    }

    #[test]
    fn test_query_format_parse() {
        assert_eq!(QueryFormat::parse("table").unwrap(), QueryFormat::Table);
        assert_eq!(
            QueryFormat::parse("markdown").unwrap(),
            QueryFormat::Markdown
        );
        assert_eq!(QueryFormat::parse("json").unwrap(), QueryFormat::Json);
        let err = QueryFormat::parse("html").unwrap_err();
        assert_eq!(
            err.to_string(),
            "Invalid format 'html'. Supported: table, markdown, json"
        );
    }

    #[test]
    fn test_query_format_parse_for_context() {
        assert_eq!(
            QueryFormat::parse_for_context("markdown").unwrap(),
            QueryFormat::Markdown
        );
        assert_eq!(
            QueryFormat::parse_for_context("json").unwrap(),
            QueryFormat::Json
        );
        let err = QueryFormat::parse_for_context("table").unwrap_err();
        assert_eq!(
            err.to_string(),
            "Invalid output format 'table'. Supported formats are: markdown, json"
        );
    }

    fn sample_target() -> SymbolInfo {
        SymbolInfo {
            id: "src/main.rs::main".to_string(),
            name: "main".to_string(),
            kind: "Function".to_string(),
            start_line: 10,
            end_line: 15,
            signature: "fn main()".to_string(),
        }
    }

    fn sample_edges() -> Vec<CallerInfo> {
        vec![CallerInfo {
            id: "src/lib.rs::init".to_string(),
            name: "init".to_string(),
            kind: "Function".to_string(),
            signature: "fn init()".to_string(),
            call_site_line: 12,
        }]
    }

    #[test]
    fn test_render_caller_callee_json() {
        let target = sample_target();
        let edges = sample_edges();
        let mut buf = Vec::new();
        QueryFormat::Json
            .render_caller_callee(&target, &edges, "Callers of `X`", "callers", &mut buf)
            .unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("\"callers\""));
        assert!(out.contains("src/main.rs::main"));
        assert!(out.contains("src/lib.rs::init"));
    }

    #[test]
    fn test_render_caller_callee_markdown() {
        let target = sample_target();
        let edges = sample_edges();
        let mut buf = Vec::new();
        QueryFormat::Markdown
            .render_caller_callee(&target, &edges, "Callers of `X`", "callers", &mut buf)
            .unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("# Callers of `X`"));
        assert!(out.contains("`src/lib.rs::init`"));
    }

    #[test]
    fn test_render_caller_callee_table() {
        let target = sample_target();
        let edges = sample_edges();
        let mut buf = Vec::new();
        QueryFormat::Table
            .render_caller_callee(&target, &edges, "Callers of `X`", "callers", &mut buf)
            .unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("Symbol ID"));
        assert!(out.contains("src/lib.rs::init"));
    }

    #[test]
    fn test_render_dependencies_all_formats() {
        let target = FileInfo {
            path: "src/main.rs".to_string(),
            language: "Rust".to_string(),
        };
        let imports = vec!["src/lib.rs".to_string()];
        let imported_by = vec!["src/bin.rs".to_string()];

        let mut buf = Vec::new();
        QueryFormat::Json
            .render_dependencies(&target, &imports, &imported_by, &mut buf)
            .unwrap();
        assert!(String::from_utf8(buf).unwrap().contains("\"imports\""));

        let mut buf = Vec::new();
        QueryFormat::Markdown
            .render_dependencies(&target, &imports, &imported_by, &mut buf)
            .unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("# Dependencies of `src/main.rs`"));
        assert!(out.contains("## Imports"));
        assert!(out.contains("## Imported By"));

        let mut buf = Vec::new();
        QueryFormat::Table
            .render_dependencies(&target, &imports, &imported_by, &mut buf)
            .unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("File Path"));
        assert!(out.contains("Direction"));
    }

    #[test]
    fn test_render_context_json() {
        let payload = ContextPayload {
            symbol: Some(sample_target()),
            file: None,
            source_code: String::new(),
            callers: vec![],
            callees: vec![],
            imports: vec![],
            imported_by: vec![],
            contained_symbols: vec![],
        };
        let mut buf = Vec::new();
        QueryFormat::Json
            .render_context(&payload, Some("src/main.rs"), &mut buf)
            .unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("\"symbol\""));
        assert!(out.contains("src/main.rs::main"));
    }

    #[test]
    fn test_render_context_markdown() {
        let payload = ContextPayload {
            symbol: Some(sample_target()),
            file: None,
            source_code: "fn main() {}".to_string(),
            callers: sample_edges(),
            callees: vec![],
            imports: vec!["src/lib.rs".to_string()],
            imported_by: vec![],
            contained_symbols: vec![],
        };
        let mut buf = Vec::new();
        QueryFormat::Markdown
            .render_context(&payload, Some("src/main.rs"), &mut buf)
            .unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("# Context: src/main.rs::main (Function)"));
        assert!(out.contains("## Signature"));
        assert!(out.contains("```rust"));
        assert!(out.contains("## Call Graph"));
        assert!(out.contains("### Callers"));
        assert!(out.contains("src/lib.rs::init"));
        assert!(out.contains("## File Dependencies"));
        assert!(out.contains("src/lib.rs"));
    }
}
