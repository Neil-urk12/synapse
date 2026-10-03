// CLI command — stdout is the output. Migrating to `tracing` is tracked
// in `docs/audits/2026-06-15-rust-best-practices-audit.md` Finding 12.
#![allow(clippy::print_stdout)]

use ignore::WalkBuilder;
use lbug::{Connection, Database, SystemConfig, Value};
use std::collections::HashMap;
use std::path::PathBuf;

#[cfg(test)]
use crate::schema;
use crate::types::db::ParsedPayload;

/// Cypher statements shared by the index writer thread (`run_index`) and the
/// watch batch processor (`process_batch`). Centralized here so a schema change
/// only needs to be made in one place.
pub const FILE_UPSERT_CYPHER: &str = "MERGE (f:File {path: $path}) \
 ON CREATE SET f.language = $language, f.file_size = $file_size, f.hash = $hash, f.raw_imports = $raw_imports \
 ON MATCH SET f.language = $language, f.file_size = $file_size, f.hash = $hash, f.raw_imports = $raw_imports";

pub const DELETE_SYMBOLS_CYPHER: &str =
    "MATCH (f:File {path: $path})-[:CONTAINS*1..]->(s:Symbol) DETACH DELETE s";

pub const SYMBOL_CREATE_CYPHER: &str = "MERGE (s:Symbol {id: $id}) \
 ON CREATE SET s.name = $name, s.kind = $kind, s.start_line = $start_line, s.start_col = $start_col, s.end_line = $end_line, s.signature = $signature, s.raw_calls = $raw_calls \
 ON MATCH SET s.name = $name, s.kind = $kind, s.start_line = $start_line, s.start_col = $start_col, s.end_line = $end_line, s.signature = $signature, s.raw_calls = $raw_calls";

pub const CONTAINMENT_FILE_CYPHER: &str =
    "MATCH (f:File {path: $from_id}), (s:Symbol {id: $to_id}) MERGE (f)-[:CONTAINS]->(s)";

pub const CONTAINMENT_SYMBOL_CYPHER: &str =
    "MATCH (p:Symbol {id: $from_id}), (c:Symbol {id: $to_id}) MERGE (p)-[:CONTAINS]->(c)";

pub struct PreparedStatements<'a> {
    pub file_upsert: &'a mut lbug::PreparedStatement,
    pub delete_symbols: &'a mut lbug::PreparedStatement,
    pub symbol_create: &'a mut lbug::PreparedStatement,
    pub containment_file: &'a mut lbug::PreparedStatement,
    pub containment_symbol: &'a mut lbug::PreparedStatement,
}

pub fn load_all_hashes(
    conn: &Connection,
) -> Result<HashMap<String, String>, Box<dyn std::error::Error>> {
    let mut cache = HashMap::new();
    let mut stmt = conn.prepare("MATCH (f:File) RETURN f.path, f.hash")?;
    let result = conn.execute(&mut stmt, vec![])?;
    for row in result {
        if let (Some(Value::String(path)), Some(Value::String(hash))) = (row.first(), row.get(1)) {
            cache.insert(path.clone(), hash.clone());
        }
    }
    Ok(cache)
}

pub fn write_payload_to_db(
    conn: &Connection,
    payload: ParsedPayload,
    stmts: &mut PreparedStatements,
    verbose: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    crate::vector::invalidate(conn)?;
    crate::database::transaction(conn, || {
        crate::database::set_dirty(conn, false, true)?;
        write_payload_inner(conn, payload, stmts, verbose)
    })
}

fn write_payload_inner(
    conn: &Connection,
    payload: ParsedPayload,
    stmts: &mut PreparedStatements,
    verbose: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let raw_imports_str = if let Some(ref analysis) = payload.analysis {
        serde_json::to_string(&analysis.imports).unwrap_or_else(|err| {
            eprintln!(
                "Warning: Failed to serialize imports for '{}': {}",
                payload.relative_path, err
            );
            "[]".to_string()
        })
    } else {
        "[]".to_string()
    };

    let file_params: Vec<(&str, Value)> = vec![
        ("path", Value::String(payload.relative_path.clone())),
        ("language", Value::String(payload.language.clone())),
        ("file_size", Value::Int64(payload.size as i64)),
        ("hash", Value::String(payload.hash)),
        ("raw_imports", Value::String(raw_imports_str)),
    ];
    conn.execute(stmts.file_upsert, file_params)?;

    crate::chunker::delete_chunks(conn, &payload.relative_path)?;

    let cleanup_params: Vec<(&str, Value)> =
        vec![("path", Value::String(payload.relative_path.clone()))];
    conn.execute(stmts.delete_symbols, cleanup_params)?;

    if let (Some(analysis), Some(content)) = (payload.analysis, payload.content) {
        for node in &analysis.nodes {
            let mut raw_calls_str = "[]".to_string();
            let symbol_calls: Vec<crate::types::ast::RawCall> = analysis
                .calls
                .iter()
                .filter(|c| c.owner_symbol_id.as_deref() == Some(node.id.as_str()))
                .map(|c| c.call.clone())
                .collect();
            match serde_json::to_string(&symbol_calls) {
                Ok(serialized) => raw_calls_str = serialized,
                Err(err) => eprintln!(
                    "Warning: Failed to serialize calls for '{}::{}': {}",
                    payload.relative_path, node.name, err
                ),
            }

            let node_params: Vec<(&str, Value)> = vec![
                ("id", Value::String(node.id.clone())),
                ("name", Value::String(node.name.clone())),
                ("kind", Value::String(node.kind.clone())),
                ("start_line", Value::Int64(node.start_line as i64)),
                ("start_col", Value::Int64(node.start_col as i64)),
                ("end_line", Value::Int64(node.end_line as i64)),
                ("signature", Value::String(node.signature.clone())),
                ("raw_calls", Value::String(raw_calls_str)),
            ];
            conn.execute(stmts.symbol_create, node_params)?;
        }

        for edge in &analysis.edges {
            let edge_params: Vec<(&str, Value)> = vec![
                ("from_id", Value::String(edge.from_id.clone())),
                ("to_id", Value::String(edge.to_id.clone())),
            ];
            if edge.from_id == payload.relative_path {
                conn.execute(stmts.containment_file, edge_params)?;
            } else {
                conn.execute(stmts.containment_symbol, edge_params)?;
            };
        }

        let chunks =
            crate::chunker::chunk_source(&payload.relative_path, &content, &analysis.nodes);
        crate::chunker::insert_chunks(conn, &payload.relative_path, &payload.language, &chunks)?;
    }

    if verbose {
        println!("Indexed: {} [{}]", payload.relative_path, payload.language);
    }
    Ok(())
}

/// Index a repository with one shared database and transactional file writes.
pub fn run_index(
    path: PathBuf,
    db_path: PathBuf,
    verbose: bool,
    no_register: bool,
    force_register: bool,
) -> crate::database::DbResult<()> {
    run_index_inner(path, db_path, verbose, no_register, force_register, &[])
}

pub(crate) fn run_index_inner(
    path: PathBuf,
    db_path: PathBuf,
    verbose: bool,
    no_register: bool,
    force_register: bool,
    excluded: &[PathBuf],
) -> crate::database::DbResult<()> {
    use std::collections::HashSet;
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    };
    let root = path.canonicalize()?;
    if !root.is_dir() {
        return Err(format!("Workspace '{}' must be a directory", root.display()).into());
    }
    if let Some(parent) = db_path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let db = Arc::new(Database::new(&db_path, SystemConfig::default())?);
    let conn = Connection::new(&db)?;
    crate::database::initialize(&conn, &root)?;
    let hash_cache = load_all_hashes(&conn)?;
    let mut excluded = excluded.to_vec();
    excluded.push(db_path.canonicalize()?);
    let (tx, rx) = std::sync::mpsc::sync_channel::<ParsedPayload>(100);
    let alive = Arc::new(AtomicBool::new(true));
    struct WriterHealth(Arc<AtomicBool>);
    impl Drop for WriterHealth {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }
    let writer_db = Arc::clone(&db);
    let writer_alive = Arc::clone(&alive);
    let writer = std::thread::spawn(move || -> Result<(u64, u64, Vec<String>), String> {
        let _health = WriterHealth(writer_alive);
        let conn = Connection::new(&writer_db).map_err(|e| e.to_string())?;
        let mut file_upsert = conn
            .prepare(FILE_UPSERT_CYPHER)
            .map_err(|e| e.to_string())?;
        let mut delete_symbols = conn
            .prepare(DELETE_SYMBOLS_CYPHER)
            .map_err(|e| e.to_string())?;
        let mut symbol_create = conn
            .prepare(SYMBOL_CREATE_CYPHER)
            .map_err(|e| e.to_string())?;
        let mut containment_file = conn
            .prepare(CONTAINMENT_FILE_CYPHER)
            .map_err(|e| e.to_string())?;
        let mut containment_symbol = conn
            .prepare(CONTAINMENT_SYMBOL_CYPHER)
            .map_err(|e| e.to_string())?;
        let mut stmts = PreparedStatements {
            file_upsert: &mut file_upsert,
            delete_symbols: &mut delete_symbols,
            symbol_create: &mut symbol_create,
            containment_file: &mut containment_file,
            containment_symbol: &mut containment_symbol,
        };
        let mut files = 0;
        let mut bytes = 0;
        let mut failures = Vec::new();
        while let Ok(payload) = rx.recv() {
            let size = payload.size;
            let name = payload.relative_path.clone();
            match write_payload_to_db(&conn, payload, &mut stmts, verbose) {
                Ok(()) => {
                    files += 1;
                    bytes += size;
                }
                Err(e) => {
                    if e.downcast_ref::<crate::database::UnusableConnection>()
                        .is_some()
                    {
                        return Err(e.to_string());
                    }
                    failures.push(format!("Failed to index '{name}': {e}"));
                }
            }
        }
        Ok((files, bytes, failures))
    });
    let observed = Mutex::new(HashSet::new());
    let failures = Mutex::new(Vec::new());
    let skipped = AtomicUsize::new(0);
    WalkBuilder::new(&root).build_parallel().run(|| {
        let tx = tx.clone();
        let alive = &alive;
        let root = &root;
        let hash_cache = &hash_cache;
        let excluded = &excluded;
        let observed = &observed;
        let failures = &failures;
        let skipped = &skipped;
        Box::new(move |entry| {
            if !alive.load(Ordering::SeqCst) {
                return ignore::WalkState::Quit;
            }
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    failures
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(e.to_string());
                    return ignore::WalkState::Continue;
                }
            };
            let file = entry.path();
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                return ignore::WalkState::Continue;
            }
            if excluded.iter().any(|db| {
                file == db
                    || file
                        .as_os_str()
                        .to_string_lossy()
                        .starts_with(&format!("{}.", db.display()))
            }) {
                return ignore::WalkState::Continue;
            }
            let relative = file
                .strip_prefix(root)
                .unwrap_or(file)
                .to_string_lossy()
                .into_owned();
            observed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(relative);
            match crate::processor::process_file(file, root, hash_cache) {
                Ok(Some(payload)) => {
                    if tx.send(payload).is_err() {
                        return ignore::WalkState::Quit;
                    }
                }
                Ok(None) => {
                    skipped.fetch_add(1, Ordering::SeqCst);
                }
                Err(e) => failures
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(format!("{}: {e}", file.display())),
            }
            ignore::WalkState::Continue
        })
    });
    drop(tx);
    let (files, bytes, writer_failures) = writer
        .join()
        .map_err(|_| "Database writer panicked; indexing failed")?
        .map_err(|e| format!("Database writer failed: {e}"))?;
    let mut failures = failures.into_inner().unwrap_or_else(|e| e.into_inner());
    failures.extend(writer_failures);
    if failures.is_empty() {
        let observed = observed.into_inner().unwrap_or_else(|e| e.into_inner());
        for stale in hash_cache.keys().filter(|p| !observed.contains(*p)) {
            remove_file(&conn, stale)?;
        }
    }
    crate::post_index::run(
        &db_path,
        verbose,
        crate::post_index::PostIndexMode::FailFast,
    )?;
    if !failures.is_empty() {
        return Err(failures.join("\n").into());
    }
    if !no_register {
        register(&root, &db_path, force_register)?;
    }
    println!(
        "Indexed/updated {files} files ({} unchanged, {bytes} bytes)",
        skipped.load(Ordering::SeqCst)
    );
    Ok(())
}

/// Remove a file and every owned symbol/chunk together.
pub fn remove_file(conn: &Connection, path: &str) -> crate::database::DbResult<()> {
    crate::vector::invalidate(conn)?;
    crate::database::transaction(conn, || {
        crate::database::set_dirty(conn, false, true)?;
        crate::chunker::delete_chunks(conn, path)?;
        let mut symbols = conn.prepare(DELETE_SYMBOLS_CYPHER)?;
        conn.execute(
            &mut symbols,
            vec![("path", Value::String(path.to_string()))],
        )?;
        let mut file = conn.prepare("MATCH (f:File {path: $path}) DETACH DELETE f")?;
        conn.execute(&mut file, vec![("path", Value::String(path.to_string()))])?;
        Ok(())
    })
}

pub(crate) fn register(
    root: &std::path::Path,
    db: &std::path::Path,
    force: bool,
) -> crate::database::DbResult<()> {
    let indexed_commit = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    crate::mcp::repo_registry::RepoRegistry::register(
        crate::mcp::repo_registry::RepoEntry {
            name: root
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unnamed")
                .to_string(),
            path: root.to_path_buf(),
            db_path: db.canonicalize()?,
            indexed_at: chrono::Utc::now().to_rfc3339(),
            indexed_commit,
        },
        force,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lbug::{Connection, Database};

    // Serialize tests that mutate the HOME env var (the registry reads
    // `$HOME/.synapse/repos.json`). Without this, parallel tests race on the
    // global env and write to each other's tempdirs.
    static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_write_payload_db(f: impl FnOnce(&Connection, &mut PreparedStatements)) {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.lbug");
        let db = Database::new(&db_path, SystemConfig::default()).unwrap();
        let conn = Connection::new(&db).unwrap();
        schema::init_schema(&conn).unwrap();

        let mut file_upsert = conn.prepare(
            "MERGE (f:File {path: $path}) \
             ON CREATE SET f.language = $language, f.file_size = $file_size, f.hash = $hash, f.raw_imports = $raw_imports \
             ON MATCH SET f.language = $language, f.file_size = $file_size, f.hash = $hash, f.raw_imports = $raw_imports"
        ).unwrap();
        let mut delete_symbols = conn
            .prepare("MATCH (f:File {path: $path})-[:CONTAINS*1..]->(s:Symbol) DETACH DELETE s")
            .unwrap();
        let mut symbol_create = conn.prepare(
            "MERGE (s:Symbol {id: $id}) \
             ON CREATE SET s.name = $name, s.kind = $kind, s.start_line = $start_line, s.start_col = $start_col, s.end_line = $end_line, s.signature = $signature, s.raw_calls = $raw_calls \
             ON MATCH SET s.name = $name, s.kind = $kind, s.start_line = $start_line, s.start_col = $start_col, s.end_line = $end_line, s.signature = $signature, s.raw_calls = $raw_calls"
        ).unwrap();
        let mut containment_file = conn.prepare(
            "MATCH (f:File {path: $from_id}), (s:Symbol {id: $to_id}) MERGE (f)-[:CONTAINS]->(s)"
        ).unwrap();
        let mut containment_symbol = conn.prepare(
            "MATCH (p:Symbol {id: $from_id}), (c:Symbol {id: $to_id}) MERGE (p)-[:CONTAINS]->(c)"
        ).unwrap();

        let mut stmts = PreparedStatements {
            file_upsert: &mut file_upsert,
            delete_symbols: &mut delete_symbols,
            symbol_create: &mut symbol_create,
            containment_file: &mut containment_file,
            containment_symbol: &mut containment_symbol,
        };

        f(&conn, &mut stmts);
    }

    #[test]
    fn test_load_all_hashes_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.lbug");
        let db = Database::new(&db_path, SystemConfig::default()).unwrap();
        let conn = Connection::new(&db).unwrap();
        schema::init_schema(&conn).unwrap();
        let res = load_all_hashes(&conn).unwrap();
        assert!(res.is_empty());
    }

    #[test]
    fn test_load_all_hashes_returns_empty_on_error() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.lbug");
        let db = Database::new(&db_path, SystemConfig::default()).unwrap();
        let conn = Connection::new(&db).unwrap();
        // No schema init — File table doesn't exist
        let result = load_all_hashes(&conn);
        assert!(
            result.is_err(),
            "Expected error when File table missing, got Ok"
        );
    }

    #[test]
    fn test_write_payload_to_db_compiles() {
        with_write_payload_db(|conn, stmts| {
            let payload = ParsedPayload {
                relative_path: "src/dummy.rs".to_string(),
                language: "Rust".to_string(),
                size: 100,
                hash: "dummyhash".to_string(),
                analysis: None,
                content: None,
            };
            let res = write_payload_to_db(conn, payload, stmts, false);
            assert!(res.is_ok());
            let mut stmt = conn
                .prepare("MATCH (f:File {path: 'src/dummy.rs'}) RETURN f.hash")
                .unwrap();
            let mut result = conn.execute(&mut stmt, vec![]).unwrap();
            let row = result.next().unwrap();
            assert_eq!(
                row.first().unwrap(),
                &Value::String("dummyhash".to_string())
            );
        });
    }

    #[test]
    fn test_write_payload_raw_imports_defaults_to_empty_array() {
        with_write_payload_db(|conn, stmts| {
            let payload = ParsedPayload {
                relative_path: "src/no_analysis.rs".to_string(),
                language: "Rust".to_string(),
                size: 50,
                hash: "abc".to_string(),
                analysis: None,
                content: None,
            };
            let res = write_payload_to_db(conn, payload, stmts, false);
            assert!(res.is_ok());
            let mut stmt = conn
                .prepare("MATCH (f:File {path: 'src/no_analysis.rs'}) RETURN f.raw_imports")
                .unwrap();
            let mut result = conn.execute(&mut stmt, vec![]).unwrap();
            let row = result.next().unwrap();
            assert_eq!(row.first().unwrap(), &Value::String("[]".to_string()));
        });
    }

    #[test]
    fn failed_replacement_rolls_back_hash_symbols_and_chunks_then_retries() {
        with_write_payload_db(|conn, stmts| {
            let payload = |source: &str, hash: &str| ParsedPayload {
                relative_path: "x.rs".into(),
                language: "Rust".into(),
                size: 100,
                hash: hash.into(),
                analysis: Some(crate::parser::ASTParser::parse_file(
                    std::path::Path::new("x.rs"),
                    source,
                )),
                content: Some(source.into()),
            };
            write_payload_to_db(conn, payload("fn old(){}", "old"), stmts, false).unwrap();
            let mut fault = conn.prepare("UNWIND [1,1] AS n CREATE (:Symbol {id: $id, name: $name, kind: $kind, start_line: $start_line, start_col: $start_col, end_line: $end_line, signature: $signature, raw_calls: $raw_calls})").unwrap();
            {
                let mut failing = PreparedStatements {
                    file_upsert: stmts.file_upsert,
                    delete_symbols: stmts.delete_symbols,
                    symbol_create: &mut fault,
                    containment_file: stmts.containment_file,
                    containment_symbol: stmts.containment_symbol,
                };
                let error =
                    write_payload_to_db(conn, payload("fn new(){}", "new"), &mut failing, false)
                        .unwrap_err();
                assert!(
                    error
                        .downcast_ref::<crate::database::UnusableConnection>()
                        .is_none(),
                    "{error}"
                );
            }
            assert_eq!(load_all_hashes(conn).unwrap()["x.rs"], "old");
            assert_eq!(
                conn.query("MATCH (s:Symbol) RETURN s.name")
                    .unwrap()
                    .next()
                    .unwrap()[0],
                Value::String("old".into())
            );
            assert_eq!(
                conn.query("MATCH (c:Chunk) RETURN c.text")
                    .unwrap()
                    .next()
                    .unwrap()[0],
                Value::String("fn old(){}".into())
            );
            write_payload_to_db(conn, payload("fn new(){}", "new"), stmts, false).unwrap();
            assert_eq!(load_all_hashes(conn).unwrap()["x.rs"], "new");
            assert_eq!(
                conn.query("MATCH (s:Symbol) RETURN s.name")
                    .unwrap()
                    .next()
                    .unwrap()[0],
                Value::String("new".into())
            );
        });
    }

    #[test]
    fn writer_startup_failure_is_reported_without_hanging_walker() {
        let workspace = tempfile::tempdir().unwrap();
        for i in 0..50 {
            std::fs::write(
                workspace.path().join(format!("f_{}.rs", i)),
                "pub fn g() {}\n",
            )
            .unwrap();
        }
        let db_path = workspace.path().join("test.lbug");
        {
            let db = Database::new(&db_path, SystemConfig::default()).unwrap();
            let conn = Connection::new(&db).unwrap();
            crate::database::initialize(&conn, workspace.path()).unwrap();
            conn.query("DROP TABLE CONTAINS").unwrap();
        }
        let error = run_index(
            workspace.path().to_path_buf(),
            db_path.clone(),
            false,
            true,
            false,
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("Database writer failed"),
            "{error}"
        );
        let db = Database::new(&db_path, SystemConfig::default()).unwrap();
        let conn = Connection::new(&db).unwrap();
        assert!(load_all_hashes(&conn).unwrap().is_empty());
    }

    fn make_git_repo(path: &std::path::Path) {
        use std::process::Command;
        Command::new("git")
            .arg("-C")
            .arg(path)
            .arg("init")
            .output()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(path)
            .arg("config")
            .arg("user.email")
            .arg("test@example.com")
            .output()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(path)
            .arg("config")
            .arg("user.name")
            .arg("Test")
            .output()
            .unwrap();
        std::fs::write(path.join("README.md"), "# Test\n").unwrap();
        Command::new("git")
            .arg("-C")
            .arg(path)
            .arg("add")
            .arg(".")
            .output()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(path)
            .arg("commit")
            .arg("-m")
            .arg("init")
            .output()
            .unwrap();
    }

    #[test]
    fn auto_register_after_index_writes_registry_entry() {
        use crate::mcp::repo_registry::RepoRegistry;
        let _home_guard = HOME_LOCK.lock().unwrap();

        let repo_dir = tempfile::tempdir().unwrap();
        let home_dir = tempfile::tempdir().unwrap();
        make_git_repo(repo_dir.path());

        // Override HOME so the registry writes into the test tempdir instead
        // of the user's real ~/.synapse/repos.json.
        let prev_home = std::env::var_os("HOME");
        std::env::set_var("HOME", home_dir.path());

        let db = repo_dir.path().join("synapse.lbug");
        let result = run_index(repo_dir.path().to_path_buf(), db, false, false, false);

        match prev_home {
            Some(prev) => std::env::set_var("HOME", prev),
            None => std::env::remove_var("HOME"),
        }

        assert!(result.is_ok(), "index failed: {:?}", result.err());

        let registry_path = home_dir.path().join(".synapse").join("repos.json");
        assert!(registry_path.exists(), "registry file not created");
        let registry = RepoRegistry::load_from(&registry_path).unwrap();
        assert_eq!(registry.entries.len(), 1);
        let entry = &registry.entries[0];
        assert!(
            !entry.indexed_commit.is_empty(),
            "indexed_commit should be set for git repo"
        );
    }

    #[test]
    fn no_register_flag_skips_registry_write() {
        use crate::mcp::repo_registry::RepoRegistry;
        let _home_guard = HOME_LOCK.lock().unwrap();

        let repo_dir = tempfile::tempdir().unwrap();
        let home_dir = tempfile::tempdir().unwrap();
        make_git_repo(repo_dir.path());

        let prev_home = std::env::var_os("HOME");
        std::env::set_var("HOME", home_dir.path());

        let db = repo_dir.path().join("synapse.lbug");
        let result = run_index(repo_dir.path().to_path_buf(), db, false, true, false);

        match prev_home {
            Some(prev) => std::env::set_var("HOME", prev),
            None => std::env::remove_var("HOME"),
        }

        assert!(result.is_ok(), "index failed: {:?}", result.err());

        let registry_path = home_dir.path().join(".synapse").join("repos.json");
        assert!(
            !registry_path.exists(),
            "registry should not be created when --no-register is passed"
        );
        // Read from the tempdir; `load()` (no args) reads the real $HOME.
        let loaded = RepoRegistry::load_from(&registry_path);
        assert!(loaded.is_ok());
        assert!(loaded.unwrap().entries.is_empty());
    }
}
