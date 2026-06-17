// CLI command — stdout is the output. Migrating to `tracing` is tracked
// in `docs/audits/2026-06-15-rust-best-practices-audit.md` Finding 12.
#![allow(clippy::print_stdout)]

use crate::resolver::{self};
use crate::types::db::{FileRecord, SymbolRecord};
use lbug::{Connection, Value};

pub fn run_linker(conn: &Connection, verbose: bool) -> Result<(), Box<dyn std::error::Error>> {
    if verbose {
        println!("🔗 Starting Global Linking Phase...");
    }

    // 1. Fetch File and Symbol caches
    let mut files = Vec::new();
    let file_query = conn.query("MATCH (f:File) RETURN f.path, f.raw_imports")?;
    for row in file_query {
        if let (Some(Value::String(path)), Some(Value::String(raw_imports))) =
            (row.first(), row.get(1))
        {
            files.push(FileRecord {
                path: path.clone(),
                raw_imports: raw_imports.clone(),
            });
        }
    }

    let mut symbols = Vec::new();
    let sym_query = conn.query("MATCH (s:Symbol) RETURN s.id, s.name, s.kind, s.raw_calls")?;
    for row in sym_query {
        if let (
            Some(Value::String(id)),
            Some(Value::String(name)),
            Some(Value::String(kind)),
            Some(Value::String(raw_calls)),
        ) = (row.first(), row.get(1), row.get(2), row.get(3))
        {
            symbols.push(SymbolRecord {
                id: id.clone(),
                name: name.clone(),
                kind: kind.clone(),
                raw_calls: raw_calls.clone(),
            });
        }
    }

    // 2. Resolve Imports & Calls
    let import_edges = resolver::resolve_imports(&files);
    let call_edges = resolver::resolve_calls(&symbols, &import_edges);

    // 3. Clear existing relationship edges
    if verbose {
        println!("🗑️  Clearing existing IMPORTS and CALLS edges...");
    }
    conn.query("MATCH (f1:File)-[r:IMPORTS]->(f2:File) DELETE r")?;
    conn.query("MATCH (s1:Symbol)-[r:CALLS]->(s2:Symbol) DELETE r")?;

    // 4. Batch insert resolved IMPORTS
    if verbose {
        println!("📝 Writing {} IMPORTS relationships...", import_edges.len());
    }
    let mut prepared_import = conn.prepare(
        "MATCH (f1:File {path: $from_path}), (f2:File {path: $to_path}) CREATE (f1)-[:IMPORTS]->(f2)"
    )?;
    for (from_path, to_path) in import_edges {
        let params: Vec<(&str, Value)> = vec![
            ("from_path", Value::String(from_path)),
            ("to_path", Value::String(to_path)),
        ];
        conn.execute(&mut prepared_import, params)?;
    }

    // 5. Batch insert resolved CALLS
    if verbose {
        println!("📝 Writing {} CALLS relationships...", call_edges.len());
    }
    let mut prepared_call = conn.prepare(
        "MATCH (s1:Symbol {id: $from_id}), (s2:Symbol {id: $to_id}) CREATE (s1)-[:CALLS {call_site_line: $call_site_line}]->(s2)"
    )?;
    for (from_id, to_id, line) in call_edges {
        let params: Vec<(&str, Value)> = vec![
            ("from_id", Value::String(from_id)),
            ("to_id", Value::String(to_id)),
            ("call_site_line", Value::Int64(line as i64)),
        ];
        conn.execute(&mut prepared_call, params)?;
    }

    if verbose {
        println!("✅ Global Linking Phase complete!");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ast::{RawCall, RawImport};
    use lbug::{Connection, Database, SystemConfig};
    use std::path::Path;

    #[test]
    fn test_global_linking() {
        let db_path = Path::new("test_linker.lbug");
        if db_path.exists() {
            let _ = std::fs::remove_file(db_path);
        }

        let db = Database::new(db_path, SystemConfig::default()).unwrap();
        let conn = Connection::new(&db).unwrap();

        // 1. Create tables
        conn.query("CREATE NODE TABLE File (path STRING, language STRING, file_size INT64, hash STRING, raw_imports STRING, PRIMARY KEY (path))").unwrap();
        conn.query("CREATE NODE TABLE Symbol (id STRING, name STRING, kind STRING, start_line INT64, start_col INT64, end_line INT64, signature STRING, raw_calls STRING, PRIMARY KEY (id))").unwrap();
        conn.query("CREATE REL TABLE CONTAINS (FROM File TO Symbol, FROM Symbol TO Symbol)")
            .unwrap();
        conn.query("CREATE REL TABLE IMPORTS (FROM File TO File)")
            .unwrap();
        conn.query("CREATE REL TABLE CALLS (FROM Symbol TO Symbol, call_site_line INT64)")
            .unwrap();

        // 2. Insert dummy File nodes
        let mut file_stmt = conn.prepare("CREATE (f:File {path: $path, language: 'Rust', file_size: 100, hash: 'abc', raw_imports: $raw_imports})").unwrap();

        let file_a_imports = serde_json::to_string(&vec![RawImport {
            path: "crate::parser".to_string(),
            line: 2,
        }])
        .unwrap();
        conn.execute(
            &mut file_stmt,
            vec![
                ("path", Value::String("src/main.rs".to_string())),
                ("raw_imports", Value::String(file_a_imports)),
            ],
        )
        .unwrap();

        let file_b_imports = serde_json::to_string(&vec![
            RawImport {
                path: "super::db".to_string(),
                line: 1,
            },
            RawImport {
                path: "main".to_string(),
                line: 2,
            },
        ])
        .unwrap();
        conn.execute(
            &mut file_stmt,
            vec![
                ("path", Value::String("src/parser.rs".to_string())),
                ("raw_imports", Value::String(file_b_imports)),
            ],
        )
        .unwrap();

        // 3. Insert dummy Symbol nodes
        let mut sym_stmt = conn.prepare("CREATE (s:Symbol {id: $id, name: $name, kind: $kind, start_line: 10, start_col: 1, end_line: 20, signature: 'fn', raw_calls: $raw_calls})").unwrap();

        let calls_main = serde_json::to_string(&vec![RawCall {
            name: "parse_file".to_string(),
            line: 12,
            is_method: false,
        }])
        .unwrap();
        conn.execute(
            &mut sym_stmt,
            vec![
                ("id", Value::String("src/main.rs::main".to_string())),
                ("name", Value::String("main".to_string())),
                ("kind", Value::String("Function".to_string())),
                ("raw_calls", Value::String(calls_main)),
            ],
        )
        .unwrap();

        let calls_parser = serde_json::to_string(&Vec::<RawCall>::new()).unwrap();
        conn.execute(
            &mut sym_stmt,
            vec![
                ("id", Value::String("src/parser.rs::parse_file".to_string())),
                ("name", Value::String("parse_file".to_string())),
                ("kind", Value::String("Function".to_string())),
                ("raw_calls", Value::String(calls_parser)),
            ],
        )
        .unwrap();

        // 4. Run linker
        run_linker(&conn, true).unwrap();

        // 5. Query resolved IMPORTS edges
        let import_query = conn
            .query("MATCH (f1:File)-[:IMPORTS]->(f2:File) RETURN f1.path, f2.path")
            .unwrap();
        let mut import_a_to_b_found = false;
        let mut import_b_to_a_found = false;
        for row in import_query {
            if let (Some(Value::String(from)), Some(Value::String(to))) = (row.first(), row.get(1))
            {
                if from == "src/main.rs" && to == "src/parser.rs" {
                    import_a_to_b_found = true;
                }
                if from == "src/parser.rs" && to == "src/main.rs" {
                    import_b_to_a_found = true;
                }
            }
        }
        assert!(
            import_a_to_b_found,
            "IMPORTS edge from src/main.rs to src/parser.rs was not created correctly"
        );
        assert!(import_b_to_a_found, "IMPORTS edge from src/parser.rs to src/main.rs (single-segment) was not created correctly");

        // 6. Query resolved CALLS edges
        let call_query = conn
            .query("MATCH (s1:Symbol)-[r:CALLS]->(s2:Symbol) RETURN s1.id, s2.id, r.call_site_line")
            .unwrap();
        let mut call_found = false;
        for row in call_query {
            if let (Some(Value::String(from)), Some(Value::String(to)), Some(Value::Int64(line))) =
                (row.first(), row.get(1), row.get(2))
            {
                if from == "src/main.rs::main" && to == "src/parser.rs::parse_file" && *line == 12 {
                    call_found = true;
                }
            }
        }
        assert!(call_found, "CALLS edge was not created correctly");

        // Cleanup
        let _ = std::fs::remove_file(db_path);
    }
}
