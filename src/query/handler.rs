use crate::query::candidates::{resolve_file_candidates, resolve_symbol_candidates};
use crate::query::db::{
    fetch_callees, fetch_callers, fetch_contained_symbols, fetch_imported_by, fetch_imports,
};
use crate::query::format::QueryFormat;
use crate::types::query::{ContextPayload, FileInfo, SymbolInfo};
use lbug::{Connection, Value};
use std::io::Write;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Callers,
    Callees,
}

/// Resolve a symbol query (callers, callees, context) against the database.
/// Fetches all symbols, filters by fuzzy/exact match, returns the first hit.
/// When `warn_multiple` is true, prints a warning to stderr if more than one
/// symbol matches (preserves `run_context`'s pre-unification behavior).
fn resolve_one_symbol(
    conn: &Connection,
    target: &str,
    exact: bool,
    warn_multiple: bool,
) -> Result<SymbolInfo, Box<dyn std::error::Error>> {
    let all_symbols = fetch_all_symbols(conn)?;
    let candidates = resolve_symbol_candidates(target, !exact, &all_symbols);
    if candidates.is_empty() {
        return Err(format!("Symbol '{}' not found", target).into());
    }
    if warn_multiple && candidates.len() > 1 {
        eprintln!("Warning: Multiple matches found for symbol '{}':", target);
        for c in &candidates {
            eprintln!("  - {}", c.id);
        }
        eprintln!("Showing details for the first match: {}", candidates[0].id);
    }
    Ok(candidates.into_iter().next().unwrap())
}

/// Resolve a file query (dependencies, file context) against the database.
fn resolve_one_file(
    conn: &Connection,
    target: &str,
    exact: bool,
    warn_multiple: bool,
) -> Result<FileInfo, Box<dyn std::error::Error>> {
    let all_files = fetch_all_files(conn)?;
    let candidates = resolve_file_candidates(target, !exact, &all_files);
    if candidates.is_empty() {
        return Err(format!("File '{}' not found", target).into());
    }
    if warn_multiple && candidates.len() > 1 {
        eprintln!("Warning: Multiple matches found for file '{}':", target);
        for c in &candidates {
            eprintln!("  - {}", c.path);
        }
        eprintln!(
            "Showing details for the first match: {}",
            candidates[0].path
        );
    }
    Ok(candidates.into_iter().next().unwrap())
}

fn fetch_all_symbols(conn: &Connection) -> Result<Vec<SymbolInfo>, Box<dyn std::error::Error>> {
    let mut stmt = conn.prepare(
        "MATCH (s:Symbol) RETURN s.id, s.name, s.kind, s.start_line, s.end_line, s.signature",
    )?;
    let query_res = conn.execute(&mut stmt, vec![])?;
    let mut all_symbols = Vec::new();
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
            all_symbols.push(SymbolInfo {
                id: id.clone(),
                name: name.clone(),
                kind: kind.clone(),
                start_line: *sl as usize,
                end_line: *el as usize,
                signature: sig.clone(),
            });
        }
    }
    Ok(all_symbols)
}

fn fetch_all_files(conn: &Connection) -> Result<Vec<FileInfo>, Box<dyn std::error::Error>> {
    let mut stmt = conn.prepare("MATCH (f:File) RETURN f.path, f.language")?;
    let query_res = conn.execute(&mut stmt, vec![])?;
    let mut all_files = Vec::new();
    for row in query_res {
        if let (Some(Value::String(path)), Some(Value::String(lang))) = (row.first(), row.get(1)) {
            all_files.push(FileInfo {
                path: path.clone(),
                language: lang.clone(),
            });
        }
    }
    Ok(all_files)
}

/// Find all callers or callees of a target symbol. The `Direction` enum
/// captures the only difference between the two CLI subcommands.
pub fn run_call_graph(
    conn: &Connection,
    symbol: &str,
    exact: bool,
    direction: Direction,
    format: QueryFormat,
    writer: &mut dyn Write,
) -> Result<(), Box<dyn std::error::Error>> {
    let target = resolve_one_symbol(conn, symbol, exact, false)?;
    let edges: Vec<crate::types::query::CallerInfo> = match direction {
        Direction::Callers => fetch_callers(conn, &target.id)?,
        Direction::Callees => fetch_callees(conn, &target.id)?
            .into_iter()
            .map(Into::into)
            .collect(),
    };
    let (heading, json_key) = match direction {
        Direction::Callers => (format!("Callers of `{}`", target.id), "callers"),
        Direction::Callees => (format!("Callees of `{}`", target.id), "callees"),
    };
    format.render_caller_callee(&target, &edges, &heading, json_key, writer)
}

/// List all dependencies (imports and imported-by) of a target file.
pub fn run_dependencies(
    conn: &Connection,
    file: &str,
    exact: bool,
    format: QueryFormat,
    writer: &mut dyn Write,
) -> Result<(), Box<dyn std::error::Error>> {
    let target = resolve_one_file(conn, file, exact, false)?;
    let imports = fetch_imports(conn, &target.path)?;
    let imported_by = fetch_imported_by(conn, &target.path)?;
    format.render_dependencies(&target, &imports, &imported_by, writer)
}

/// Retrieve consolidated code intelligence for a symbol or file: signature,
/// source code slice, call graph, and file dependencies. Either `symbol` or
/// `file` must be specified (mutually exclusive).
pub fn run_context(
    conn: &Connection,
    symbol: Option<&str>,
    file: Option<&str>,
    fuzzy: bool,
    format: QueryFormat,
    writer: &mut dyn Write,
) -> Result<(), Box<dyn std::error::Error>> {
    if symbol.is_some() && file.is_some() {
        return Err("Options --symbol and --file are mutually exclusive".into());
    }
    if symbol.is_none() && file.is_none() {
        return Err("Either --symbol or --file must be specified".into());
    }

    let mut target_symbol: Option<SymbolInfo> = None;
    let mut target_file: Option<FileInfo> = None;
    let mut file_path_to_read: Option<String> = None;
    let mut line_range: Option<(usize, usize)> = None;
    let mut callers = Vec::new();
    let mut callees = Vec::new();
    let mut imports = Vec::new();
    let mut imported_by = Vec::new();
    let mut contained_symbols = Vec::new();
    let mut source_code = String::new();

    if let Some(sym) = symbol {
        let resolved = resolve_one_symbol(conn, sym, !fuzzy, true)?;
        target_symbol = Some(resolved.clone());
    } else if let Some(fl) = file {
        let resolved = resolve_one_file(conn, fl, !fuzzy, true)?;
        target_file = Some(resolved.clone());
    }

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
            source_code = match line_range {
                Some((start, end)) => crate::file_utils::slice_source_code(&content, start, end),
                None => content,
            };
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

    format.render_context(&payload, file_path_to_read.as_deref(), writer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lbug::{Connection, Database, SystemConfig};
    use std::path::Path;

    /// Populate an open connection with the schema and minimal data for tests.
    /// The caller owns the Database and Connection lifetimes.
    fn setup_test_db(conn: &Connection) {
        conn.query(
            "CREATE NODE TABLE File(path STRING, language STRING, file_size INT64, hash STRING, raw_imports STRING, PRIMARY KEY(path))"
        ).unwrap();
        conn.query(
            "CREATE NODE TABLE Symbol(id STRING, name STRING, kind STRING, start_line INT64, start_col INT64, end_line INT64, signature STRING, raw_calls STRING, PRIMARY KEY(id))"
        ).unwrap();
        conn.query("CREATE REL TABLE CONTAINS(FROM File TO Symbol, FROM Symbol TO Symbol)")
            .unwrap();
        conn.query("CREATE REL TABLE IMPORTS(FROM File TO File)")
            .unwrap();
        conn.query("CREATE REL TABLE CALLS(FROM Symbol TO Symbol, call_site_line INT64)")
            .unwrap();
        conn.query("CREATE (:File {path: 'src/main.rs', language: 'Rust', file_size: 100, hash: 'h1', raw_imports: '[]'})")
            .unwrap();
        conn.query("CREATE (:File {path: 'src/parser.rs', language: 'Rust', file_size: 150, hash: 'h2', raw_imports: '[]'})")
            .unwrap();
        conn.query("MATCH (f1:File {path: 'src/main.rs'}), (f2:File {path: 'src/parser.rs'}) CREATE (f1)-[:IMPORTS]->(f2)")
            .unwrap();
        conn.query("CREATE (:Symbol {id: 'src/main.rs::main', name: 'main', kind: 'Function', start_line: 10, start_col: 1, end_line: 15, signature: 'fn main()', raw_calls: '[]'})")
            .unwrap();
        conn.query("CREATE (:Symbol {id: 'src/parser.rs::parse', name: 'parse', kind: 'Function', start_line: 20, start_col: 1, end_line: 25, signature: 'fn parse()', raw_calls: '[]'})")
            .unwrap();
        conn.query("MATCH (s1:Symbol {id: 'src/main.rs::main'}), (s2:Symbol {id: 'src/parser.rs::parse'}) CREATE (s1)-[:CALLS {call_site_line: 12}]->(s2)")
            .unwrap();
    }

    #[test]
    fn test_run_call_graph_callers_and_callees() {
        let db_path = Path::new("test_handler_callgraph.lbug");
        if db_path.exists() {
            let _ = std::fs::remove_file(db_path);
        }
        let db = Database::new(db_path, SystemConfig::default()).unwrap();
        let conn = Connection::new(&db).unwrap();
        setup_test_db(&conn);

        // Callers of `parse` → main is the caller
        let mut out = Vec::new();
        run_call_graph(
            &conn,
            "parse",
            false,
            Direction::Callers,
            QueryFormat::Table,
            &mut out,
        )
        .unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("src/main.rs::main"));
        assert!(s.contains("12"));

        // Callees of `main` → parse is the callee
        let mut out = Vec::new();
        run_call_graph(
            &conn,
            "main",
            false,
            Direction::Callees,
            QueryFormat::Table,
            &mut out,
        )
        .unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("src/parser.rs::parse"));

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn test_run_call_graph_json_key_varies_with_direction() {
        let db_path = Path::new("test_handler_callgraph_json.lbug");
        if db_path.exists() {
            let _ = std::fs::remove_file(db_path);
        }
        let db = Database::new(db_path, SystemConfig::default()).unwrap();
        let conn = Connection::new(&db).unwrap();
        setup_test_db(&conn);

        let mut out = Vec::new();
        run_call_graph(
            &conn,
            "parse",
            true,
            Direction::Callers,
            QueryFormat::Json,
            &mut out,
        )
        .unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("\"callers\""));
        assert!(!s.contains("\"callees\""));

        let mut out = Vec::new();
        run_call_graph(
            &conn,
            "main",
            true,
            Direction::Callees,
            QueryFormat::Json,
            &mut out,
        )
        .unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("\"callees\""));
        assert!(!s.contains("\"callers\""));

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn test_run_dependencies_table_and_json() {
        let db_path = Path::new("test_handler_deps.lbug");
        if db_path.exists() {
            let _ = std::fs::remove_file(db_path);
        }
        let db = Database::new(db_path, SystemConfig::default()).unwrap();
        let conn = Connection::new(&db).unwrap();
        setup_test_db(&conn);

        let mut out = Vec::new();
        run_dependencies(&conn, "main.rs", false, QueryFormat::Table, &mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("src/parser.rs"));
        assert!(s.contains("Imports"));

        let mut out = Vec::new();
        run_dependencies(&conn, "main.rs", false, QueryFormat::Json, &mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("\"imports\""));
        assert!(s.contains("\"imported_by\""));

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn test_run_context_symbol_markdown() {
        let db_path = Path::new("test_handler_context.lbug");
        if db_path.exists() {
            let _ = std::fs::remove_file(db_path);
        }
        let db = Database::new(db_path, SystemConfig::default()).unwrap();
        let conn = Connection::new(&db).unwrap();
        setup_test_db(&conn);

        let mut out = Vec::new();
        run_context(
            &conn,
            Some("main"),
            None,
            false,
            QueryFormat::Markdown,
            &mut out,
        )
        .unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("Context: src/main.rs::main"));
        assert!(s.contains("Function"));

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn test_run_context_table_falls_back_to_markdown() {
        let db_path = Path::new("test_handler_context_table.lbug");
        if db_path.exists() {
            let _ = std::fs::remove_file(db_path);
        }
        let db = Database::new(db_path, SystemConfig::default()).unwrap();
        let conn = Connection::new(&db).unwrap();
        setup_test_db(&conn);

        // run_context takes a QueryFormat directly. If a caller passes Table
        // (bypassing parse_for_context), render_context defensively falls back
        // to markdown. The proper rejection happens at the parse_for_context
        // boundary, which is tested in format.rs.
        let mut out = Vec::new();
        let res = run_context(
            &conn,
            Some("main"),
            None,
            false,
            QueryFormat::Table,
            &mut out,
        );
        // We don't error — the defensive fallback writes markdown-shaped output.
        assert!(res.is_ok());
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("Context: src/main.rs::main"));

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn test_run_context_rejects_both_symbol_and_file() {
        let db_path = Path::new("test_handler_context_both.lbug");
        if db_path.exists() {
            let _ = std::fs::remove_file(db_path);
        }
        let db = Database::new(db_path, SystemConfig::default()).unwrap();
        let conn = Connection::new(&db).unwrap();
        setup_test_db(&conn);

        let mut out = Vec::new();
        let res = run_context(
            &conn,
            Some("main"),
            Some("main.rs"),
            false,
            QueryFormat::Markdown,
            &mut out,
        );
        assert!(res.is_err());
        assert_eq!(
            res.unwrap_err().to_string(),
            "Options --symbol and --file are mutually exclusive"
        );

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn test_run_context_rejects_neither_symbol_nor_file() {
        let db_path = Path::new("test_handler_context_neither.lbug");
        if db_path.exists() {
            let _ = std::fs::remove_file(db_path);
        }
        let db = Database::new(db_path, SystemConfig::default()).unwrap();
        let conn = Connection::new(&db).unwrap();
        setup_test_db(&conn);

        let mut out = Vec::new();
        let res = run_context(&conn, None, None, false, QueryFormat::Markdown, &mut out);
        assert!(res.is_err());
        assert_eq!(
            res.unwrap_err().to_string(),
            "Either --symbol or --file must be specified"
        );

        let _ = std::fs::remove_file(db_path);
    }
}
