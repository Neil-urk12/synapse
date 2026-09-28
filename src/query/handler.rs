use crate::query::candidates::{resolve_file_candidates, resolve_symbol_candidates};
use crate::query::db::{
    fetch_callees, fetch_callers, fetch_contained_symbols, fetch_imported_by, fetch_imports,
};
use crate::query::error::{CandidateItem, QueryError};
use crate::query::format::QueryFormat;
pub use crate::types::query::{
    CallGraphResult, CallerInfo, ContextPayload, DependenciesResult, Direction, FileInfo,
    SymbolInfo,
};
use lbug::{Connection, Value};
use std::error::Error;
use std::io::Write;

/// Resolve a symbol query (callers, callees, context) against the database.
/// Fetches all symbols, filters by fuzzy/exact match, returns the first hit.
/// When `warn_multiple` is true and more than one symbol matches, returns
/// `QueryError::Ambiguous` carrying the full structured candidate list.
pub fn resolve_one_symbol(
    conn: &Connection,
    target: &str,
    exact: bool,
    warn_multiple: bool,
) -> Result<SymbolInfo, QueryError> {
    let all_symbols = fetch_all_symbols(conn)?;
    let candidates = resolve_symbol_candidates(target, !exact, &all_symbols);
    if candidates.is_empty() {
        return Err(QueryError::NotFound(target.to_string()));
    }
    if warn_multiple && candidates.len() > 1 {
        let payload: Vec<CandidateItem> = candidates
            .iter()
            .map(|c| {
                let file = c.id.split("::").next().unwrap_or(&c.id).to_string();
                CandidateItem {
                    name: c.name.clone(),
                    file,
                    line: c.start_line,
                }
            })
            .collect();
        return Err(QueryError::Ambiguous {
            target: target.to_string(),
            candidates: payload,
        });
    }
    Ok(candidates.into_iter().next().unwrap())
}

/// Resolve a file query (dependencies, file context) against the database.
/// When `warn_multiple` is true and more than one file matches, returns
/// `QueryError::Ambiguous`. Files don't carry line numbers in the graph schema,
/// so `line` is reported as 0 for each file candidate.
pub fn resolve_one_file(
    conn: &Connection,
    target: &str,
    exact: bool,
    warn_multiple: bool,
) -> Result<FileInfo, QueryError> {
    let all_files = fetch_all_files(conn)?;
    let candidates = resolve_file_candidates(target, !exact, &all_files);
    if candidates.is_empty() {
        return Err(QueryError::NotFound(target.to_string()));
    }
    if warn_multiple && candidates.len() > 1 {
        let payload: Vec<CandidateItem> = candidates
            .iter()
            .map(|c| CandidateItem {
                name: c.path.clone(),
                file: c.path.clone(),
                line: 0,
            })
            .collect();
        return Err(QueryError::Ambiguous {
            target: target.to_string(),
            candidates: payload,
        });
    }
    Ok(candidates.into_iter().next().unwrap())
}

pub fn fetch_all_symbols(conn: &Connection) -> Result<Vec<SymbolInfo>, QueryError> {
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
                start_line: usize::try_from(*sl).unwrap_or(0),
                end_line: usize::try_from(*el).unwrap_or(0),
                signature: sig.clone(),
            });
        }
    }
    Ok(all_symbols)
}

pub fn fetch_all_files(conn: &Connection) -> Result<Vec<FileInfo>, QueryError> {
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

/// Execute a call graph query, finding callers or callees of a target symbol.
pub fn query_call_graph(
    conn: &Connection,
    symbol: &str,
    exact: bool,
    direction: Direction,
) -> Result<CallGraphResult, QueryError> {
    let target = resolve_one_symbol(conn, symbol, exact, !exact)?;
    let edges: Vec<CallerInfo> = match direction {
        Direction::Callers => {
            fetch_callers(conn, &target.id).map_err(|e| QueryError::Database(e.to_string()))?
        }
        Direction::Callees => fetch_callees(conn, &target.id)
            .map_err(|e| QueryError::Database(e.to_string()))?
            .into_iter()
            .map(Into::into)
            .collect(),
    };
    Ok(CallGraphResult {
        target,
        edges,
        direction,
    })
}

/// Execute a dependency query, listing imports and imported-by files for a target file.
pub fn query_dependencies(
    conn: &Connection,
    file: &str,
    exact: bool,
) -> Result<DependenciesResult, QueryError> {
    let target = resolve_one_file(conn, file, exact, !exact)?;
    let imports =
        fetch_imports(conn, &target.path).map_err(|e| QueryError::Database(e.to_string()))?;
    let imported_by =
        fetch_imported_by(conn, &target.path).map_err(|e| QueryError::Database(e.to_string()))?;
    Ok(DependenciesResult {
        file: target.path,
        imports,
        imported_by,
    })
}

/// Execute a context query, retrieving consolidated code intelligence for a symbol or file.
pub fn query_context(
    conn: &Connection,
    symbol: Option<&str>,
    file: Option<&str>,
    fuzzy: bool,
) -> Result<ContextPayload, QueryError> {
    if symbol.is_some() && file.is_some() {
        return Err(QueryError::InvalidInput(
            "Options --symbol and --file are mutually exclusive".into(),
        ));
    }
    if symbol.is_none() && file.is_none() {
        return Err(QueryError::InvalidInput(
            "Either --symbol or --file must be specified".into(),
        ));
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
        target_symbol = Some(resolved);
    } else if let Some(fl) = file {
        let resolved = resolve_one_file(conn, fl, !fuzzy, true)?;
        target_file = Some(resolved);
    }

    if let Some(ref sym) = target_symbol {
        callers = fetch_callers(conn, &sym.id).map_err(|e| QueryError::Database(e.to_string()))?;
        callees = fetch_callees(conn, &sym.id).map_err(|e| QueryError::Database(e.to_string()))?;
        if let Some(first_seg) = sym.id.split("::").next() {
            file_path_to_read = Some(first_seg.to_string());
            imports =
                fetch_imports(conn, first_seg).map_err(|e| QueryError::Database(e.to_string()))?;
            imported_by = fetch_imported_by(conn, first_seg)
                .map_err(|e| QueryError::Database(e.to_string()))?;
        }
        line_range = Some((sym.start_line, sym.end_line));
    } else if let Some(ref fl) = target_file {
        file_path_to_read = Some(fl.path.clone());
        imports = fetch_imports(conn, &fl.path).map_err(|e| QueryError::Database(e.to_string()))?;
        imported_by =
            fetch_imported_by(conn, &fl.path).map_err(|e| QueryError::Database(e.to_string()))?;
        contained_symbols = fetch_contained_symbols(conn, &fl.path)
            .map_err(|e| QueryError::Database(e.to_string()))?;
    }

    if let Some(ref path_str) = file_path_to_read {
        if let Ok(content) = std::fs::read_to_string(path_str) {
            source_code = match line_range {
                Some((start, end)) => crate::file_utils::slice_source_code(&content, start, end),
                None => content,
            };
        }
    }

    Ok(ContextPayload {
        symbol: target_symbol,
        file: target_file,
        source_code,
        callers,
        callees,
        imports,
        imported_by,
        contained_symbols,
    })
}

// ─── Legacy/CLI Presentation Wrappers ───────────────────────────────────────

pub fn run_call_graph(
    conn: &Connection,
    symbol: &str,
    exact: bool,
    direction: Direction,
    format: QueryFormat,
    writer: &mut dyn Write,
) -> Result<(), Box<dyn Error>> {
    let result = match query_call_graph(conn, symbol, exact, direction) {
        Ok(res) => res,
        Err(e) => return Err(Box::new(e) as Box<dyn Error>),
    };
    let (heading, json_key) = match direction {
        Direction::Callers => (format!("Callers of `{}`", result.target.id), "callers"),
        Direction::Callees => (format!("Callees of `{}`", result.target.id), "callees"),
    };
    format.render_caller_callee(&result.target, &result.edges, &heading, json_key, writer)
}

pub fn run_dependencies(
    conn: &Connection,
    file: &str,
    exact: bool,
    format: QueryFormat,
    writer: &mut dyn Write,
) -> Result<(), Box<dyn Error>> {
    let result = match query_dependencies(conn, file, exact) {
        Ok(res) => res,
        Err(e) => return Err(Box::new(e) as Box<dyn Error>),
    };
    let target = FileInfo {
        path: result.file.clone(),
        language: String::new(),
    };
    format.render_dependencies(&target, &result.imports, &result.imported_by, writer)
}

pub fn run_context(
    conn: &Connection,
    symbol: Option<&str>,
    file: Option<&str>,
    fuzzy: bool,
    format: QueryFormat,
    writer: &mut dyn Write,
) -> Result<(), Box<dyn Error>> {
    let payload = match query_context(conn, symbol, file, fuzzy) {
        Ok(p) => p,
        Err(e) => return Err(Box::new(e) as Box<dyn Error>),
    };
    let file_path = payload
        .symbol
        .as_ref()
        .and_then(|s| s.id.split("::").next().map(ToString::to_string))
        .or_else(|| payload.file.as_ref().map(|f| f.path.clone()));
    format.render_context(&payload, file_path.as_deref(), writer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lbug::{Connection, Database, SystemConfig};
    use tempfile::tempdir;

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
    fn test_query_call_graph_typed() {
        let tmp = tempdir().unwrap();
        let db_path = tmp.path().join("test_cg_typed.lbug");
        let db = Database::new(&db_path, SystemConfig::default()).unwrap();
        let conn = Connection::new(&db).unwrap();
        setup_test_db(&conn);

        let callers = query_call_graph(&conn, "parse", false, Direction::Callers).unwrap();
        assert_eq!(callers.target.id, "src/parser.rs::parse");
        assert_eq!(callers.edges.len(), 1);
        assert_eq!(callers.edges[0].id, "src/main.rs::main");
        assert_eq!(callers.edges[0].call_site_line, 12);

        let callees = query_call_graph(&conn, "main", false, Direction::Callees).unwrap();
        assert_eq!(callees.target.id, "src/main.rs::main");
        assert_eq!(callees.edges.len(), 1);
        assert_eq!(callees.edges[0].id, "src/parser.rs::parse");
    }

    #[test]
    fn test_query_dependencies_typed() {
        let tmp = tempdir().unwrap();
        let db_path = tmp.path().join("test_deps_typed.lbug");
        let db = Database::new(&db_path, SystemConfig::default()).unwrap();
        let conn = Connection::new(&db).unwrap();
        setup_test_db(&conn);

        let deps = query_dependencies(&conn, "main.rs", false).unwrap();
        assert_eq!(deps.file, "src/main.rs");
        assert_eq!(deps.imports, vec!["src/parser.rs"]);
        assert!(deps.imported_by.is_empty());
    }

    #[test]
    fn test_query_context_typed() {
        let tmp = tempdir().unwrap();
        let db_path = tmp.path().join("test_ctx_typed.lbug");
        let db = Database::new(&db_path, SystemConfig::default()).unwrap();
        let conn = Connection::new(&db).unwrap();
        setup_test_db(&conn);

        let ctx = query_context(&conn, Some("main"), None, false).unwrap();
        assert_eq!(ctx.symbol.unwrap().name, "main");
        assert_eq!(ctx.callees.len(), 1);
        assert_eq!(ctx.callees[0].id, "src/parser.rs::parse");
    }

    #[test]
    fn test_run_call_graph_callers_and_callees() {
        let tmp = tempdir().unwrap();
        let db_path = tmp.path().join("test_handler_callgraph.lbug");
        let db = Database::new(&db_path, SystemConfig::default()).unwrap();
        let conn = Connection::new(&db).unwrap();
        setup_test_db(&conn);

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
    }

    #[test]
    fn test_run_call_graph_json_key_varies_with_direction() {
        let tmp = tempdir().unwrap();
        let db_path = tmp.path().join("test_handler_callgraph_json.lbug");
        let db = Database::new(&db_path, SystemConfig::default()).unwrap();
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
    }

    #[test]
    fn test_run_dependencies_table_and_json() {
        let tmp = tempdir().unwrap();
        let db_path = tmp.path().join("test_handler_deps.lbug");
        let db = Database::new(&db_path, SystemConfig::default()).unwrap();
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
    }

    #[test]
    fn test_run_context_symbol_markdown() {
        let tmp = tempdir().unwrap();
        let db_path = tmp.path().join("test_handler_context.lbug");
        let db = Database::new(&db_path, SystemConfig::default()).unwrap();
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
    }

    #[test]
    fn test_run_context_rejects_both_symbol_and_file() {
        let tmp = tempdir().unwrap();
        let db_path = tmp.path().join("test_handler_context_both.lbug");
        let db = Database::new(&db_path, SystemConfig::default()).unwrap();
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
    }

    #[test]
    fn test_run_context_rejects_neither_symbol_nor_file() {
        let tmp = tempdir().unwrap();
        let db_path = tmp.path().join("test_handler_context_neither.lbug");
        let db = Database::new(&db_path, SystemConfig::default()).unwrap();
        let conn = Connection::new(&db).unwrap();
        setup_test_db(&conn);

        let mut out = Vec::new();
        let res = run_context(&conn, None, None, false, QueryFormat::Markdown, &mut out);
        assert!(res.is_err());
        assert_eq!(
            res.unwrap_err().to_string(),
            "Either --symbol or --file must be specified"
        );
    }
}
