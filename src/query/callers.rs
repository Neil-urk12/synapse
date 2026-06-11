use crate::query::candidates::resolve_symbol_candidates;
use crate::query::db::fetch_callers;
use crate::query::format::format_ascii_table;
use crate::types::query::SymbolInfo;
use lbug::{Connection, Database, SystemConfig, Value};
use std::path::Path;

pub fn run_callers_internal(
    conn: &Connection,
    symbol: &str,
    exact: bool,
    format: &str,
    writer: &mut dyn std::io::Write,
) -> Result<(), Box<dyn std::error::Error>> {
    if format != "table" && format != "markdown" && format != "json" {
        return Err(format!(
            "Invalid format '{}'. Supported: table, markdown, json",
            format
        )
        .into());
    }

    let mut stmt_all = conn.prepare(
        "MATCH (s:Symbol) RETURN s.id, s.name, s.kind, s.start_line, s.end_line, s.signature",
    )?;
    let query_all = conn.execute(&mut stmt_all, vec![])?;
    let mut all_symbols = Vec::new();
    for row in query_all {
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

    let candidates = resolve_symbol_candidates(symbol, !exact, &all_symbols);
    if candidates.is_empty() {
        return Err(format!("Symbol '{}' not found", symbol).into());
    }
    let target = &candidates[0];
    let callers = fetch_callers(conn, &target.id)?;

    if format == "json" {
        let json_val = serde_json::json!({
            "target": target,
            "callers": callers
        });
        writeln!(writer, "{}", serde_json::to_string_pretty(&json_val)?)?;
    } else if format == "markdown" {
        writeln!(writer, "# Callers of `{}`", target.id)?;
        if callers.is_empty() {
            writeln!(writer, "\nNo callers found.")?;
        } else {
            for c in &callers {
                writeln!(
                    writer,
                    "* `{}` (kind: {}, line: {})",
                    c.id, c.kind, c.call_site_line
                )?;
            }
        }
    } else {
        let headers = vec![
            "Symbol ID".to_string(),
            "Kind".to_string(),
            "Signature".to_string(),
            "Line".to_string(),
        ];
        let rows: Vec<Vec<String>> = callers
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
    Ok(())
}

pub fn handle_callers(
    symbol: &str,
    exact: bool,
    format: &str,
    db_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let db = Database::new(db_path, SystemConfig::default())?;
    let conn = Connection::new(&db)?;
    run_callers_internal(&conn, symbol, exact, format, &mut std::io::stdout())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::callees::run_callees_internal;
    use crate::query::dependencies::run_dependencies_internal;
    use lbug::{Connection, Database, SystemConfig};
    use std::path::Path;

    #[test]
    fn test_callers_callees_dependencies_presets() {
        let db_path = Path::new("test_presets.lbug");
        if db_path.exists() {
            let _ = std::fs::remove_file(db_path);
        }
        let db = Database::new(db_path, SystemConfig::default()).unwrap();
        let conn = Connection::new(&db).unwrap();

        conn.query("CREATE NODE TABLE File(path STRING, language STRING, file_size INT64, hash STRING, raw_imports STRING, PRIMARY KEY(path))").unwrap();
        conn.query("CREATE NODE TABLE Symbol(id STRING, name STRING, kind STRING, start_line INT64, start_col INT64, end_line INT64, signature STRING, raw_calls STRING, PRIMARY KEY(id))").unwrap();
        conn.query("CREATE REL TABLE CONTAINS(FROM File TO Symbol, FROM Symbol TO Symbol)").unwrap();
        conn.query("CREATE REL TABLE IMPORTS(FROM File TO File)").unwrap();
        conn.query("CREATE REL TABLE CALLS(FROM Symbol TO Symbol, call_site_line INT64)").unwrap();

        conn.query("CREATE (:File {path: 'src/main.rs', language: 'Rust', file_size: 100, hash: 'h1', raw_imports: '[]'})").unwrap();
        conn.query("CREATE (:File {path: 'src/parser.rs', language: 'Rust', file_size: 150, hash: 'h2', raw_imports: '[]'})").unwrap();
        conn.query("MATCH (f1:File {path: 'src/main.rs'}), (f2:File {path: 'src/parser.rs'}) CREATE (f1)-[:IMPORTS]->(f2)").unwrap();

        conn.query("CREATE (:Symbol {id: 'src/main.rs::main', name: 'main', kind: 'Function', start_line: 10, start_col: 1, end_line: 15, signature: 'fn main()', raw_calls: '[]'})").unwrap();
        conn.query("CREATE (:Symbol {id: 'src/parser.rs::parse', name: 'parse', kind: 'Function', start_line: 20, start_col: 1, end_line: 25, signature: 'fn parse()', raw_calls: '[]'})").unwrap();
        conn.query("MATCH (s1:Symbol {id: 'src/main.rs::main'}), (s2:Symbol {id: 'src/parser.rs::parse'}) CREATE (s1)-[:CALLS {call_site_line: 12}]->(s2)").unwrap();

        let mut output = Vec::new();
        run_callers_internal(&conn, "parse", false, "table", &mut output).unwrap();
        let out_str = String::from_utf8(output).unwrap();
        assert!(out_str.contains("src/main.rs::main"));
        assert!(out_str.contains("12"));

        let mut output_callees = Vec::new();
        run_callees_internal(&conn, "main", false, "table", &mut output_callees).unwrap();
        let out_callees_str = String::from_utf8(output_callees).unwrap();
        assert!(out_callees_str.contains("src/parser.rs::parse"));

        let mut output_deps = Vec::new();
        run_dependencies_internal(&conn, "main.rs", false, "table", &mut output_deps).unwrap();
        let out_deps_str = String::from_utf8(output_deps).unwrap();
        assert!(out_deps_str.contains("src/parser.rs"));
        assert!(out_deps_str.contains("Imports"));

        let _ = std::fs::remove_file(db_path);
    }
}
