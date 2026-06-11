use crate::query::candidates::resolve_file_candidates;
use crate::query::db::{fetch_imported_by, fetch_imports};
use crate::query::format::format_ascii_table;
use crate::types::query::FileInfo;
use lbug::{Connection, Database, SystemConfig, Value};
use std::path::Path;

pub fn run_dependencies_internal(
    conn: &Connection,
    file: &str,
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

    let mut stmt_all = conn.prepare("MATCH (f:File) RETURN f.path, f.language")?;
    let query_all = conn.execute(&mut stmt_all, vec![])?;
    let mut all_files = Vec::new();
    for row in query_all {
        if let (Some(Value::String(path)), Some(Value::String(lang))) = (row.first(), row.get(1)) {
            all_files.push(FileInfo {
                path: path.clone(),
                language: lang.clone(),
            });
        }
    }

    let candidates = resolve_file_candidates(file, !exact, &all_files);
    if candidates.is_empty() {
        return Err(format!("File '{}' not found", file).into());
    }
    let target = &candidates[0];
    let imports = fetch_imports(conn, &target.path)?;
    let imported_by = fetch_imported_by(conn, &target.path)?;

    if format == "json" {
        let json_val = serde_json::json!({
            "target": target,
            "imports": imports,
            "imported_by": imported_by
        });
        writeln!(writer, "{}", serde_json::to_string_pretty(&json_val)?)?;
    } else if format == "markdown" {
        writeln!(writer, "# Dependencies of `{}`", target.path)?;
        writeln!(writer, "\n## Imports")?;
        if imports.is_empty() {
            writeln!(writer, "No imports.")?;
        } else {
            for imp in &imports {
                writeln!(writer, "* `{}`", imp)?;
            }
        }
        writeln!(writer, "\n## Imported By")?;
        if imported_by.is_empty() {
            writeln!(writer, "No files import this file.")?;
        } else {
            for imp_by in &imported_by {
                writeln!(writer, "* `{}`", imp_by)?;
            }
        }
    } else {
        let headers = vec!["File Path".to_string(), "Direction".to_string()];
        let mut rows = Vec::new();
        for imp in &imports {
            rows.push(vec![imp.clone(), "Imports".to_string()]);
        }
        for imp_by in &imported_by {
            rows.push(vec![imp_by.clone(), "Imported By".to_string()]);
        }
        writeln!(writer, "{}", format_ascii_table(&headers, &rows))?;
    }
    Ok(())
}

pub fn handle_dependencies(
    file: &str,
    exact: bool,
    format: &str,
    db_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let db = Database::new(db_path, SystemConfig::default())?;
    let conn = Connection::new(&db)?;
    run_dependencies_internal(&conn, file, exact, format, &mut std::io::stdout())?;
    Ok(())
}
