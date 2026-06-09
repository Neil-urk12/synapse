use lbug::{Connection, Value};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
struct FileRecord {
    path: String,
    raw_imports: String,
}

#[derive(Debug, Deserialize)]
struct SymbolRecord {
    id: String,
    name: String,
    kind: String,
    raw_calls: String,
}

#[derive(Debug, serde::Serialize, Deserialize)]
struct RawImport {
    path: String,
    line: usize,
}

#[derive(Debug, serde::Serialize, Deserialize)]
struct RawCall {
    name: String,
    line: usize,
    is_method: bool,
}

fn normalize_path(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            Component::Normal(c) => {
                normalized.push(c);
            }
            Component::RootDir | Component::Prefix(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

fn resolve_imports(files: &[FileRecord]) -> Vec<(String, String)> {
    let mut edges = HashSet::new();

    let file_paths: HashMap<String, &FileRecord> =
        files.iter().map(|f| (f.path.clone(), f)).collect();

    for file in files {
        if let Ok(raw_imports) = serde_json::from_str::<Vec<RawImport>>(&file.raw_imports) {
            let base_path = Path::new(&file.path);
            let base_dir = base_path.parent().unwrap_or(Path::new(""));
            let ext = base_path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let is_rust = ext == "rs";

            for imp in raw_imports {
                let mut resolved = false;

                // JS/TS Relative Path Resolution
                if imp.path.starts_with('.') {
                    let target_raw = base_dir.join(&imp.path);
                    let target_normalized = normalize_path(&target_raw);

                    for ext in &["ts", "tsx", "js", "jsx"] {
                        let path_with_ext = target_normalized.with_extension(ext);
                        let path_str = path_with_ext.to_string_lossy().to_string();
                        if file_paths.contains_key(&path_str) {
                            edges.insert((file.path.clone(), path_str));
                            resolved = true;
                            break;
                        }
                    }

                    if !resolved {
                        for ext in &["ts", "js"] {
                            let path_with_index = target_normalized.join(format!("index.{}", ext));
                            let path_str = path_with_index.to_string_lossy().to_string();
                            if file_paths.contains_key(&path_str) {
                                edges.insert((file.path.clone(), path_str));
                                break;
                            }
                        }
                    }
                }
                // Rust Module Resolution
                else if is_rust {
                    let segments: Vec<&str> = imp.path.split("::").collect();
                    if !segments.is_empty() {
                        let mut start_idx = 0;
                        let mut resolved_base_dir = PathBuf::from("src");

                        if segments[0] == "crate" {
                            start_idx = 1;
                        } else if segments[0] == "self" {
                            start_idx = 1;
                            resolved_base_dir = base_dir.to_path_buf();
                        } else {
                            // Resolve multiple super prefixes recursively
                            let mut current_dir = base_dir;
                            while start_idx < segments.len() && segments[start_idx] == "super" {
                                current_dir = current_dir.parent().unwrap_or(Path::new(""));
                                start_idx += 1;
                                resolved_base_dir = current_dir.to_path_buf();
                            }
                        }

                        if start_idx < segments.len() {
                            let mod_name = segments[start_idx];

                            let search_paths = vec![
                                resolved_base_dir.join(format!("{}.rs", mod_name)),
                                resolved_base_dir.join(mod_name).join("mod.rs"),
                            ];

                            for p in search_paths {
                                let p_normalized = normalize_path(&p);
                                let p_str = p_normalized.to_string_lossy().to_string();
                                if file_paths.contains_key(&p_str) {
                                    edges.insert((file.path.clone(), p_str));
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    edges.into_iter().collect()
}

fn resolve_calls(
    symbols: &[SymbolRecord],
    import_edges: &[(String, String)],
) -> Vec<(String, String, usize)> {
    let mut resolved_edges = HashSet::new();

    let mut file_symbols: HashMap<String, Vec<&SymbolRecord>> = HashMap::new();
    let mut symbol_map: HashMap<String, &SymbolRecord> = HashMap::new();
    let mut name_map: HashMap<String, Vec<&SymbolRecord>> = HashMap::new();

    for sym in symbols {
        symbol_map.insert(sym.id.clone(), sym);
        name_map.entry(sym.name.clone()).or_default().push(sym);

        if let Some(file_path) = sym.id.split("::").next() {
            file_symbols
                .entry(file_path.to_string())
                .or_default()
                .push(sym);
        }
    }

    let mut file_imports: HashMap<String, Vec<String>> = HashMap::new();
    for (from_file, to_file) in import_edges {
        file_imports
            .entry(from_file.clone())
            .or_default()
            .push(to_file.clone());
    }

    for sym in symbols {
        if let Ok(raw_calls) = serde_json::from_str::<Vec<RawCall>>(&sym.raw_calls) {
            let file_path = sym.id.split("::").next().unwrap_or("");

            for call in raw_calls {
                let mut resolved = false;

                // Step 1: Surrounding Context (Method calls on classes/impl blocks)
                if call.is_method {
                    let segments: Vec<&str> = sym.id.split("::").collect();
                    if segments.len() >= 3 {
                        let parent_id = segments[..segments.len() - 1].join("::");
                        let method_id = format!("{}::{}", parent_id, call.name);
                        if symbol_map.contains_key(&method_id) {
                            resolved_edges.insert((sym.id.clone(), method_id, call.line));
                            resolved = true;
                        }
                    }
                }

                // Step 2: File Scope
                if !resolved {
                    if let Some(local_syms) = file_symbols.get(file_path) {
                        for local in local_syms {
                            if local.name == call.name {
                                resolved_edges.insert((
                                    sym.id.clone(),
                                    local.id.clone(),
                                    call.line,
                                ));
                                resolved = true;
                                break;
                            }
                        }
                    }
                }

                // Step 3: Import Scope (including nested symbols inside G)
                if !resolved {
                    if let Some(imports) = file_imports.get(file_path) {
                        for imported_file in imports {
                            if let Some(imported_syms) = file_symbols.get(imported_file) {
                                for target in imported_syms {
                                    if target.name == call.name {
                                        resolved_edges.insert((
                                            sym.id.clone(),
                                            target.id.clone(),
                                            call.line,
                                        ));
                                        resolved = true;
                                        break;
                                    }
                                }
                            }
                            if resolved {
                                break;
                            }
                        }
                    }
                }

                // Step 4: Global Fallback (matching prioritize Function and Method)
                if !resolved {
                    if let Some(matches) = name_map.get(&call.name) {
                        let mut candidates: Vec<&SymbolRecord> = matches
                            .iter()
                            .filter(|s| s.kind == "Function" || s.kind == "Method")
                            .cloned()
                            .collect();

                        if candidates.is_empty() {
                            candidates = matches.clone();
                        }

                        for candidate in candidates {
                            resolved_edges.insert((
                                sym.id.clone(),
                                candidate.id.clone(),
                                call.line,
                            ));
                        }
                    }
                }
            }
        }
    }

    resolved_edges.into_iter().collect()
}

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
    let import_edges = resolve_imports(&files);
    let call_edges = resolve_calls(&symbols, &import_edges);

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
