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
        let row_strs: Vec<String> = row
            .iter()
            .map(|val| {
                val.to_string()
                    .replace('\n', "\\n")
                    .replace('\r', "\\r")
            })
            .collect();
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
            if target.contains("::") {
                if fuzzy {
                    s.id.to_lowercase().contains(&target.to_lowercase())
                } else {
                    s.id == target
                }
            } else {
                if fuzzy {
                    s.name.to_lowercase().contains(&target.to_lowercase())
                } else {
                    s.name == target
                }
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
    let mut stmt = conn.prepare("MATCH (f:File {path: $path})-[:CONTAINS*1..]->(s:Symbol) RETURN s.id, s.name, s.kind, s.signature")?;
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


#[derive(Debug, Clone, serde::Serialize)]
pub struct ContextPayload {
    pub symbol: Option<SymbolInfo>,
    pub file: Option<FileInfo>,
    pub source_code: String,
    pub callers: Vec<CallerInfo>,
    pub callees: Vec<CalleeInfo>,
    pub imports: Vec<String>,
    pub imported_by: Vec<String>,
    pub contained_symbols: Vec<ContainedSymbolInfo>,
}

pub fn run_context_internal(
    conn: &Connection,
    symbol_name: Option<&str>,
    file_path: Option<&str>,
    fuzzy: bool,
    format: &str,
    writer: &mut dyn std::io::Write,
) -> Result<(), Box<dyn std::error::Error>> {
    if format != "markdown" && format != "json" {
        return Err(format!("Invalid output format '{}'. Supported formats are: markdown, json", format).into());
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
        let query_res = conn.execute(&mut stmt, vec![("target", Value::String(sym.to_string()))])?;
        for row in query_res {
            if let (
                Some(Value::String(id)),
                Some(Value::String(name)),
                Some(Value::String(kind)),
                Some(Value::Int64(sl)),
                Some(Value::Int64(el)),
                Some(Value::String(sig)),
            ) = (row.first(), row.get(1), row.get(2), row.get(3), row.get(4), row.get(5)) {
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
            let query_res = conn.execute(&mut stmt, vec![("target", Value::String(fl.to_string()))])?;
            for row in query_res {
                if let (Some(Value::String(path)), Some(Value::String(lang))) = (row.first(), row.get(1)) {
                    candidates.push(FileInfo {
                        path: path.clone(),
                        language: lang.clone(),
                    });
                }
            }
        } else {
            let mut stmt = conn.prepare("MATCH (f:File {path: $target}) RETURN f.path, f.language")?;
            let query_res = conn.execute(&mut stmt, vec![("target", Value::String(fl.to_string()))])?;
            for row in query_res {
                if let (Some(Value::String(path)), Some(Value::String(lang))) = (row.first(), row.get(1)) {
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
            eprintln!("Showing details for the first match: {}", candidates[0].path);
        }
        target_file = Some(candidates[0].clone());
    }

    // Collect context details
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
                source_code = slice_source_code(&content, start, end);
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
        // Markdown format
        if let Some(ref sym) = payload.symbol {
            writeln!(writer, "# Context: {} ({})", sym.id, sym.kind)?;
            writeln!(writer, "\n## Signature\n`{}`", sym.signature)?;
            if let Some(ref p) = file_path_to_read {
                writeln!(writer, "\n## Source Code ({}:{}-{})", p, sym.start_line, sym.end_line)?;
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
                    writeln!(writer, "* `{}` (kind: {}, line: {})", caller.id, caller.kind, caller.call_site_line)?;
                }
            }
            if !payload.callees.is_empty() {
                writeln!(writer, "### Callees")?;
                for callee in &payload.callees {
                    writeln!(writer, "* `{}` (kind: {}, line: {})", callee.id, callee.kind, callee.call_site_line)?;
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

pub fn run_callers_internal(
    conn: &Connection,
    symbol: &str,
    exact: bool,
    format: &str,
    writer: &mut dyn std::io::Write,
) -> Result<(), Box<dyn std::error::Error>> {
    if format != "table" && format != "markdown" && format != "json" {
        return Err(format!("Invalid format '{}'. Supported: table, markdown, json", format).into());
    }

    let mut stmt_all = conn.prepare("MATCH (s:Symbol) RETURN s.id, s.name, s.kind, s.start_line, s.end_line, s.signature")?;
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
        ) = (row.first(), row.get(1), row.get(2), row.get(3), row.get(4), row.get(5)) {
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
                writeln!(writer, "* `{}` (kind: {}, line: {})", c.id, c.kind, c.call_site_line)?;
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

pub fn run_callees_internal(
    conn: &Connection,
    symbol: &str,
    exact: bool,
    format: &str,
    writer: &mut dyn std::io::Write,
) -> Result<(), Box<dyn std::error::Error>> {
    if format != "table" && format != "markdown" && format != "json" {
        return Err(format!("Invalid format '{}'. Supported: table, markdown, json", format).into());
    }

    let mut stmt_all = conn.prepare("MATCH (s:Symbol) RETURN s.id, s.name, s.kind, s.start_line, s.end_line, s.signature")?;
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
        ) = (row.first(), row.get(1), row.get(2), row.get(3), row.get(4), row.get(5)) {
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
                writeln!(writer, "* `{}` (kind: {}, line: {})", c.id, c.kind, c.call_site_line)?;
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

pub fn run_dependencies_internal(
    conn: &Connection,
    file: &str,
    exact: bool,
    format: &str,
    writer: &mut dyn std::io::Write,
) -> Result<(), Box<dyn std::error::Error>> {
    if format != "table" && format != "markdown" && format != "json" {
        return Err(format!("Invalid format '{}'. Supported: table, markdown, json", format).into());
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
        let headers = vec![
            "File Path".to_string(),
            "Direction".to_string(),
        ];
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

        // Exact symbol ID match
        let matches_id = resolve_symbol_candidates("src/linker.rs::run_linker", false, &symbols);
        assert_eq!(matches_id.len(), 1);
        assert_eq!(matches_id[0].id, "src/linker.rs::run_linker");

        // Fuzzy symbol ID match
        let matches_id_fuzzy = resolve_symbol_candidates("linker.rs::run", true, &symbols);
        assert_eq!(matches_id_fuzzy.len(), 1);
        assert_eq!(matches_id_fuzzy[0].id, "src/linker.rs::run_linker");
    }

    #[test]
    fn test_source_line_slicing() {
        let file_content = "line 1\nline 2\nline 3\nline 4\nline 5";
        let sliced = slice_source_code(file_content, 2, 4);
        assert_eq!(sliced, "line 2\nline 3\nline 4");
    }

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
        conn.query("CREATE REL TABLE CONTAINS (FROM File TO Symbol, FROM Symbol TO Symbol)").unwrap();
        conn.query("CREATE REL TABLE IMPORTS (FROM File TO File)").unwrap();
        conn.query("CREATE REL TABLE CALLS (FROM Symbol TO Symbol, call_site_line INT64)").unwrap();
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
        assert_eq!(res.unwrap_err().to_string(), "Invalid output format 'html'. Supported formats are: markdown, json");
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn test_callers_callees_dependencies_presets() {
        let db_path = Path::new("test_presets.lbug");
        if db_path.exists() {
            let _ = std::fs::remove_file(db_path);
        }
        let db = Database::new(db_path, SystemConfig::default()).unwrap();
        let conn = Connection::new(&db).unwrap();

        // DDL
        conn.query("CREATE NODE TABLE File(path STRING, language STRING, file_size INT64, hash STRING, raw_imports STRING, PRIMARY KEY(path))").unwrap();
        conn.query("CREATE NODE TABLE Symbol(id STRING, name STRING, kind STRING, start_line INT64, start_col INT64, end_line INT64, signature STRING, raw_calls STRING, PRIMARY KEY(id))").unwrap();
        conn.query("CREATE REL TABLE CONTAINS(FROM File TO Symbol, FROM Symbol TO Symbol)").unwrap();
        conn.query("CREATE REL TABLE IMPORTS(FROM File TO File)").unwrap();
        conn.query("CREATE REL TABLE CALLS(FROM Symbol TO Symbol, call_site_line INT64)").unwrap();

        // Seed Data
        conn.query("CREATE (:File {path: 'src/main.rs', language: 'Rust', file_size: 100, hash: 'h1', raw_imports: '[]'})").unwrap();
        conn.query("CREATE (:File {path: 'src/parser.rs', language: 'Rust', file_size: 150, hash: 'h2', raw_imports: '[]'})").unwrap();
        conn.query("MATCH (f1:File {path: 'src/main.rs'}), (f2:File {path: 'src/parser.rs'}) CREATE (f1)-[:IMPORTS]->(f2)").unwrap();

        conn.query("CREATE (:Symbol {id: 'src/main.rs::main', name: 'main', kind: 'Function', start_line: 10, start_col: 1, end_line: 15, signature: 'fn main()', raw_calls: '[]'})").unwrap();
        conn.query("CREATE (:Symbol {id: 'src/parser.rs::parse', name: 'parse', kind: 'Function', start_line: 20, start_col: 1, end_line: 25, signature: 'fn parse()', raw_calls: '[]'})").unwrap();
        conn.query("MATCH (s1:Symbol {id: 'src/main.rs::main'}), (s2:Symbol {id: 'src/parser.rs::parse'}) CREATE (s1)-[:CALLS {call_site_line: 12}]->(s2)").unwrap();

        // Test Callers
        let mut output = Vec::new();
        run_callers_internal(&conn, "parse", false, "table", &mut output).unwrap();
        let out_str = String::from_utf8(output).unwrap();
        assert!(out_str.contains("src/main.rs::main"));
        assert!(out_str.contains("12"));

        // Test Callees
        let mut output_callees = Vec::new();
        run_callees_internal(&conn, "main", false, "table", &mut output_callees).unwrap();
        let out_callees_str = String::from_utf8(output_callees).unwrap();
        assert!(out_callees_str.contains("src/parser.rs::parse"));

        // Test Dependencies
        let mut output_deps = Vec::new();
        run_dependencies_internal(&conn, "main.rs", false, "table", &mut output_deps).unwrap();
        let out_deps_str = String::from_utf8(output_deps).unwrap();
        assert!(out_deps_str.contains("src/parser.rs"));
        assert!(out_deps_str.contains("Imports"));

        let _ = std::fs::remove_file(db_path);
    }
}

