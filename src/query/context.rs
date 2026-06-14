use crate::query::db::{
    fetch_callees, fetch_callers, fetch_contained_symbols, fetch_imported_by, fetch_imports,
};
use crate::types::query::{ContextPayload, FileInfo, SymbolInfo};
use lbug::{Connection, Database, SystemConfig, Value};
use std::path::Path;

pub fn run_context_internal(
    conn: &Connection,
    symbol_name: Option<&str>,
    file_path: Option<&str>,
    fuzzy: bool,
    format: &str,
    writer: &mut dyn std::io::Write,
) -> Result<(), Box<dyn std::error::Error>> {
    if format != "markdown" && format != "json" {
        return Err(format!(
            "Invalid output format '{}'. Supported formats are: markdown, json",
            format
        )
        .into());
    }
    if symbol_name.is_some() && file_path.is_some() {
        return Err("Options --symbol and --file are mutually exclusive".into());
    }
    if symbol_name.is_none() && file_path.is_none() {
        return Err("Either --symbol or --file must be specified".into());
    }

    let mut target_symbol = None;
    let mut target_file = None;

    if let Some(sym) = symbol_name {
        let mut candidates = Vec::new();
        let query_str = if sym.contains("::") {
            if fuzzy {
                "MATCH (s:Symbol) WHERE LOWER(s.id) CONTAINS LOWER($target) RETURN s.id, s.name, s.kind, s.start_line, s.end_line, s.signature"
            } else {
                "MATCH (s:Symbol {id: $target}) RETURN s.id, s.name, s.kind, s.start_line, s.end_line, s.signature"
            }
        } else {
            if fuzzy {
                "MATCH (s:Symbol) WHERE LOWER(s.name) CONTAINS LOWER($target) RETURN s.id, s.name, s.kind, s.start_line, s.end_line, s.signature"
            } else {
                "MATCH (s:Symbol {name: $target}) RETURN s.id, s.name, s.kind, s.start_line, s.end_line, s.signature"
            }
        };
        let mut stmt = conn.prepare(query_str)?;
        let query_res =
            conn.execute(&mut stmt, vec![("target", Value::String(sym.to_string()))])?;
        for row in query_res {
            if let (
                Some(Value::String(id)),
                Some(Value::String(name)),
                Some(Value::String(kind)),
                Some(Value::Int64(sl)),
                Some(Value::Int64(el)),
                Some(Value::String(sig)),
            ) = (
                row.first(),
                row.get(1),
                row.get(2),
                row.get(3),
                row.get(4),
                row.get(5),
            ) {
                candidates.push(SymbolInfo {
                    id: id.clone(),
                    name: name.clone(),
                    kind: kind.clone(),
                    start_line: *sl as usize,
                    end_line: *el as usize,
                    signature: sig.clone(),
                });
            }
        }

        if candidates.is_empty() {
            return Err(format!("Symbol '{}' not found", sym).into());
        }
        if candidates.len() > 1 {
            eprintln!("Warning: Multiple matches found for symbol '{}':", sym);
            for c in &candidates {
                eprintln!("  - {}", c.id);
            }
            eprintln!("Showing details for the first match: {}", candidates[0].id);
        }
        target_symbol = Some(candidates[0].clone());
    } else if let Some(fl) = file_path {
        let mut candidates = Vec::new();
        if fuzzy {
            let mut stmt = conn.prepare("MATCH (f:File) WHERE LOWER(f.path) CONTAINS LOWER($target) RETURN f.path, f.language")?;
            let query_res =
                conn.execute(&mut stmt, vec![("target", Value::String(fl.to_string()))])?;
            for row in query_res {
                if let (Some(Value::String(path)), Some(Value::String(lang))) =
                    (row.first(), row.get(1))
                {
                    candidates.push(FileInfo {
                        path: path.clone(),
                        language: lang.clone(),
                    });
                }
            }
        } else {
            let mut stmt =
                conn.prepare("MATCH (f:File {path: $target}) RETURN f.path, f.language")?;
            let query_res =
                conn.execute(&mut stmt, vec![("target", Value::String(fl.to_string()))])?;
            for row in query_res {
                if let (Some(Value::String(path)), Some(Value::String(lang))) =
                    (row.first(), row.get(1))
                {
                    candidates.push(FileInfo {
                        path: path.clone(),
                        language: lang.clone(),
                    });
                }
            }
        }

        if candidates.is_empty() {
            return Err(format!("File '{}' not found", fl).into());
        }
        if candidates.len() > 1 {
            eprintln!("Warning: Multiple matches found for file '{}':", fl);
            for c in &candidates {
                eprintln!("  - {}", c.path);
            }
            eprintln!(
                "Showing details for the first match: {}",
                candidates[0].path
            );
        }
        target_file = Some(candidates[0].clone());
    }

    let mut callers = Vec::new();
    let mut callees = Vec::new();
    let mut imports = Vec::new();
    let mut imported_by = Vec::new();
    let mut contained_symbols = Vec::new();
    let mut source_code = String::new();
    let mut file_path_to_read = None;
    let mut line_range = None;

    if let Some(ref sym) = target_symbol {
        callers = fetch_callers(conn, &sym.id)?;
        callees = fetch_callees(conn, &sym.id)?;

        if let Some(first_seg) = sym.id.split("::").next() {
            file_path_to_read = Some(first_seg.to_string());
            imports = fetch_imports(conn, first_seg)?;
            imported_by = fetch_imported_by(conn, first_seg)?;
        }
        line_range = Some((sym.start_line, sym.end_line));
    } else if let Some(ref fl) = target_file {
        file_path_to_read = Some(fl.path.clone());
        imports = fetch_imports(conn, &fl.path)?;
        imported_by = fetch_imported_by(conn, &fl.path)?;
        contained_symbols = fetch_contained_symbols(conn, &fl.path)?;
    }

    if let Some(ref path_str) = file_path_to_read {
        if let Ok(content) = std::fs::read_to_string(path_str) {
            if let Some((start, end)) = line_range {
                source_code = crate::file_utils::slice_source_code(&content, start, end);
            } else {
                source_code = content;
            }
        }
    }

    let payload = ContextPayload {
        symbol: target_symbol,
        file: target_file,
        source_code,
        callers,
        callees,
        imports,
        imported_by,
        contained_symbols,
    };

    if format == "json" {
        let json_str = serde_json::to_string_pretty(&payload)?;
        writeln!(writer, "{}", json_str)?;
    } else {
        if let Some(ref sym) = payload.symbol {
            writeln!(writer, "# Context: {} ({})", sym.id, sym.kind)?;
            writeln!(writer, "\n## Signature\n`{}`", sym.signature)?;
            if let Some(ref p) = file_path_to_read {
                writeln!(
                    writer,
                    "\n## Source Code ({}:{}-{})",
                    p, sym.start_line, sym.end_line
                )?;
                let syntax = if p.ends_with(".rs") {
                    "rust"
                } else if p.ends_with(".js") || p.ends_with(".jsx") {
                    "javascript"
                } else if p.ends_with(".ts") || p.ends_with(".tsx") {
                    "typescript"
                } else {
                    "text"
                };
                writeln!(writer, "```{}\n{}\n```", syntax, payload.source_code)?;
            }
        } else if let Some(ref fl) = payload.file {
            writeln!(writer, "# Context: {} ({})", fl.path, fl.language)?;
            writeln!(writer, "\n## Source Code ({})", fl.path)?;
            let syntax = if fl.path.ends_with(".rs") {
                "rust"
            } else if fl.path.ends_with(".js") || fl.path.ends_with(".jsx") {
                "javascript"
            } else if fl.path.ends_with(".ts") || fl.path.ends_with(".tsx") {
                "typescript"
            } else {
                "text"
            };
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
            let path_ctx = file_path_to_read.as_deref().unwrap_or("");
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

    Ok(())
}

pub fn handle_context(
    symbol: Option<&str>,
    file: Option<&str>,
    fuzzy: bool,
    format: &str,
    db_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let db = Database::new(db_path, SystemConfig::default())?;
    let conn = Connection::new(&db)?;
    run_context_internal(&conn, symbol, file, fuzzy, format, &mut std::io::stdout())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lbug::{Connection, Database, SystemConfig};
    use std::path::Path;

    #[test]
    fn test_handle_context_integration() {
        let db_path = Path::new("test_context_cli.lbug");
        if db_path.exists() {
            let _ = std::fs::remove_file(db_path);
        }
        let db = Database::new(db_path, SystemConfig::default()).unwrap();
        let conn = Connection::new(&db).unwrap();
        conn.query("CREATE NODE TABLE File (path STRING, language STRING, file_size INT64, hash STRING, raw_imports STRING, PRIMARY KEY (path))").unwrap();
        conn.query("CREATE NODE TABLE Symbol (id STRING, name STRING, kind STRING, start_line INT64, start_col INT64, end_line INT64, signature STRING, raw_calls STRING, PRIMARY KEY (id))").unwrap();
        conn.query("CREATE REL TABLE CONTAINS (FROM File TO Symbol, FROM Symbol TO Symbol)")
            .unwrap();
        conn.query("CREATE REL TABLE IMPORTS (FROM File TO File)")
            .unwrap();
        conn.query("CREATE REL TABLE CALLS (FROM Symbol TO Symbol, call_site_line INT64)")
            .unwrap();
        conn.query("CREATE (:File {path: 'src/main.rs', language: 'Rust', file_size: 10, hash: 'x', raw_imports: '[]'})").unwrap();
        conn.query("CREATE (:Symbol {id: 'src/main.rs::main', name: 'main', kind: 'Function', start_line: 1, start_col: 1, end_line: 2, signature: 'fn main()', raw_calls: '[]'})").unwrap();

        let mut out_buf = Vec::new();
        run_context_internal(&conn, Some("main"), None, false, "markdown", &mut out_buf).unwrap();
        let out_str = String::from_utf8(out_buf).unwrap();

        assert!(out_str.contains("Context: src/main.rs::main"));
        assert!(out_str.contains("Function"));
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn test_invalid_format() {
        let db_path = Path::new("test_format_val.lbug");
        if db_path.exists() {
            let _ = std::fs::remove_file(db_path);
        }
        let db = Database::new(db_path, SystemConfig::default()).unwrap();
        let conn = Connection::new(&db).unwrap();
        let mut out_buf = Vec::new();
        let res = run_context_internal(&conn, Some("main"), None, false, "html", &mut out_buf);
        assert!(res.is_err());
        assert_eq!(
            res.unwrap_err().to_string(),
            "Invalid output format 'html'. Supported formats are: markdown, json"
        );
        let _ = std::fs::remove_file(db_path);
    }
}
