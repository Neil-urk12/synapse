use std::path::Path;
use lbug::{Connection, Database, SystemConfig, Value};

#[derive(Debug, Clone, serde::Serialize)]
pub struct SymbolInfo {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub start_line: usize,
    pub end_line: usize,
    pub signature: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FileInfo {
    pub path: String,
    pub language: String,
}

pub fn run_query_internal(
    conn: &Connection,
    query: &str,
    writer: &mut dyn std::io::Write,
) -> Result<(), Box<dyn std::error::Error>> {
    let result = conn.query(query)?;
    let headers = result.get_column_names();
    let mut rows = Vec::new();

    for row in result {
        let row_strs: Vec<String> = row.iter().map(|val| val.to_string()).collect();
        rows.push(row_strs);
    }

    let formatted_table = format_ascii_table(&headers, &rows);
    writeln!(writer, "{}", formatted_table)?;
    Ok(())
}

pub fn handle_query(query: &str, db_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let db = Database::new(db_path, SystemConfig::default())?;
    let conn = Connection::new(&db)?;
    run_query_internal(&conn, query, &mut std::io::stdout())?;
    Ok(())
}

pub fn resolve_symbol_candidates(
    target: &str,
    fuzzy: bool,
    pool: &[SymbolInfo],
) -> Vec<SymbolInfo> {
    pool.iter()
        .filter(|s| {
            if fuzzy {
                s.name.to_lowercase().contains(&target.to_lowercase())
            } else {
                s.name == target
            }
        })
        .cloned()
        .collect()
}

pub fn resolve_file_candidates(
    target: &str,
    fuzzy: bool,
    pool: &[FileInfo],
) -> Vec<FileInfo> {
    pool.iter()
        .filter(|f| {
            if fuzzy {
                f.path.to_lowercase().contains(&target.to_lowercase())
            } else {
                f.path == target
            }
        })
        .cloned()
        .collect()
}

pub fn slice_source_code(content: &str, start_line: usize, end_line: usize) -> String {
    if start_line == 0 || end_line == 0 || start_line > end_line {
        return String::new();
    }
    let lines: Vec<&str> = content.lines().collect();
    let start_idx = start_line - 1;
    let end_idx = std::cmp::min(end_line, lines.len());
    if start_idx >= lines.len() {
        return String::new();
    }
    lines[start_idx..end_idx].join("\n")
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CallerInfo {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub signature: String,
    pub call_site_line: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CalleeInfo {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub signature: String,
    pub call_site_line: usize,
}

pub fn fetch_callers(conn: &Connection, target_id: &str) -> Result<Vec<CallerInfo>, Box<dyn std::error::Error>> {
    let mut stmt = conn.prepare(
        "MATCH (s1:Symbol)-[r:CALLS]->(s2:Symbol {id: $target_id}) \
         RETURN s1.id, s1.name, s1.kind, s1.signature, r.call_site_line"
    )?;
    let query_res = conn.execute(&mut stmt, vec![("target_id", Value::String(target_id.to_string()))])?;
    let mut callers = Vec::new();
    for row in query_res {
        if let (
            Some(Value::String(id)),
            Some(Value::String(name)),
            Some(Value::String(kind)),
            Some(Value::String(sig)),
            Some(Value::Int64(line)),
        ) = (row.first(), row.get(1), row.get(2), row.get(3), row.get(4)) {
            callers.push(CallerInfo {
                id: id.clone(),
                name: name.clone(),
                kind: kind.clone(),
                signature: sig.clone(),
                call_site_line: *line as usize,
            });
        }
    }
    Ok(callers)
}

pub fn fetch_callees(conn: &Connection, target_id: &str) -> Result<Vec<CalleeInfo>, Box<dyn std::error::Error>> {
    let mut stmt = conn.prepare(
        "MATCH (s1:Symbol {id: $target_id})-[r:CALLS]->(s2:Symbol) \
         RETURN s2.id, s2.name, s2.kind, s2.signature, r.call_site_line"
    )?;
    let query_res = conn.execute(&mut stmt, vec![("target_id", Value::String(target_id.to_string()))])?;
    let mut callees = Vec::new();
    for row in query_res {
        if let (
            Some(Value::String(id)),
            Some(Value::String(name)),
            Some(Value::String(kind)),
            Some(Value::String(sig)),
            Some(Value::Int64(line)),
        ) = (row.first(), row.get(1), row.get(2), row.get(3), row.get(4)) {
            callees.push(CalleeInfo {
                id: id.clone(),
                name: name.clone(),
                kind: kind.clone(),
                signature: sig.clone(),
                call_site_line: *line as usize,
            });
        }
    }
    Ok(callees)
}

pub fn fetch_imports(conn: &Connection, file_path: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut stmt = conn.prepare("MATCH (f1:File {path: $path})-[:IMPORTS]->(f2:File) RETURN f2.path")?;
    let query_res = conn.execute(&mut stmt, vec![("path", Value::String(file_path.to_string()))])?;
    let mut imports = Vec::new();
    for row in query_res {
        if let Some(Value::String(import_path)) = row.first() {
            imports.push(import_path.clone());
        }
    }
    Ok(imports)
}

pub fn fetch_imported_by(conn: &Connection, file_path: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut stmt = conn.prepare("MATCH (f1:File)-[:IMPORTS]->(f2:File {path: $path}) RETURN f1.path")?;
    let query_res = conn.execute(&mut stmt, vec![("path", Value::String(file_path.to_string()))])?;
    let mut imported_by = Vec::new();
    for row in query_res {
        if let Some(Value::String(imported_path)) = row.first() {
            imported_by.push(imported_path.clone());
        }
    }
    Ok(imported_by)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ContainedSymbolInfo {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub signature: String,
}

pub fn fetch_contained_symbols(conn: &Connection, file_path: &str) -> Result<Vec<ContainedSymbolInfo>, Box<dyn std::error::Error>> {
    let mut stmt = conn.prepare("MATCH (f:File {path: $path})-[:CONTAINS]->(s:Symbol) RETURN s.id, s.name, s.kind, s.signature")?;
    let query_res = conn.execute(&mut stmt, vec![("path", Value::String(file_path.to_string()))])?;
    let mut symbols = Vec::new();
    for row in query_res {
        if let (
            Some(Value::String(id)),
            Some(Value::String(name)),
            Some(Value::String(kind)),
            Some(Value::String(sig)),
        ) = (row.first(), row.get(1), row.get(2), row.get(3)) {
            symbols.push(ContainedSymbolInfo {
                id: id.clone(),
                name: name.clone(),
                kind: kind.clone(),
                signature: sig.clone(),
            });
        }
    }
    Ok(symbols)
}


pub fn handle_context(
    symbol: Option<&str>,
    file: Option<&str>,
    fuzzy: bool,
    format: &str,
    db_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("Context: symbol={:?}, file={:?}, fuzzy={}, format={}", symbol, file, fuzzy, format);
    Ok(())
}

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
        for (i, cell) in row.iter().enumerate() {
            out.push_str(&format!(" {:<width$} |", cell, width = col_widths[i]));
        }
        out.push('\n');
    }
    out.push_str(&separator.trim_end());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_handle_query_integration() {
        let db_path = Path::new("test_query_cli.lbug");
        if db_path.exists() {
            let _ = std::fs::remove_file(db_path);
        }
        let db = Database::new(db_path, SystemConfig::default()).unwrap();
        let conn = Connection::new(&db).unwrap();
        conn.query("CREATE NODE TABLE T(id INT64, name STRING, PRIMARY KEY(id))").unwrap();
        conn.query("CREATE (:T {id: 101, name: 'Indexer'})").unwrap();

        let mut output_buf = Vec::new();
        run_query_internal(&conn, "MATCH (n:T) RETURN n.id AS ID, n.name AS NAME", &mut output_buf).unwrap();
        let output = String::from_utf8(output_buf).unwrap();

        assert!(output.contains("101"));
        assert!(output.contains("Indexer"));
        assert!(output.contains("ID"));
        assert!(output.contains("NAME"));
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn test_fuzzy_filtering() {
        let symbols = vec![
            SymbolInfo {
                id: "src/main.rs::main".to_string(),
                name: "main".to_string(),
                kind: "Function".to_string(),
                start_line: 1,
                end_line: 5,
                signature: "fn main()".to_string(),
            },
            SymbolInfo {
                id: "src/linker.rs::run_linker".to_string(),
                name: "run_linker".to_string(),
                kind: "Function".to_string(),
                start_line: 10,
                end_line: 20,
                signature: "fn run_linker()".to_string(),
            },
        ];

        // Exact match
        let matches = resolve_symbol_candidates("run_linker", false, &symbols);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].id, "src/linker.rs::run_linker");

        // Fuzzy case-insensitive substring
        let matches_fuzzy = resolve_symbol_candidates("LINK", true, &symbols);
        assert_eq!(matches_fuzzy.len(), 1);
        assert_eq!(matches_fuzzy[0].id, "src/linker.rs::run_linker");
    }

    #[test]
    fn test_source_line_slicing() {
        let file_content = "line 1\nline 2\nline 3\nline 4\nline 5";
        let sliced = slice_source_code(file_content, 2, 4);
        assert_eq!(sliced, "line 2\nline 3\nline 4");
    }
}
