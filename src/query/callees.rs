use crate::query::candidates::resolve_symbol_candidates;
use crate::query::db::fetch_callees;
use crate::query::format::format_ascii_table;
use crate::types::query::SymbolInfo;
use lbug::{Connection, Database, SystemConfig, Value};
use std::path::Path;

pub fn run_callees_internal(
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
    let callees = fetch_callees(conn, &target.id)?;

    if format == "json" {
        let json_val = serde_json::json!({
            "target": target,
            "callees": callees
        });
        writeln!(writer, "{}", serde_json::to_string_pretty(&json_val)?)?;
    } else if format == "markdown" {
        writeln!(writer, "# Callees of `{}`", target.id)?;
        if callees.is_empty() {
            writeln!(writer, "\nNo callees found.")?;
        } else {
            for c in &callees {
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
        let rows: Vec<Vec<String>> = callees
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

pub fn handle_callees(
    symbol: &str,
    exact: bool,
    format: &str,
    db_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let db = Database::new(db_path, SystemConfig::default())?;
    let conn = Connection::new(&db)?;
    run_callees_internal(&conn, symbol, exact, format, &mut std::io::stdout())?;
    Ok(())
}
