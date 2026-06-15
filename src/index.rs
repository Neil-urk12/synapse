use ignore::WalkBuilder;
use lbug::{Connection, Database, SystemConfig, Value};
use std::collections::HashMap;
use std::path::PathBuf;

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

    let cleanup_params: Vec<(&str, Value)> =
        vec![("path", Value::String(payload.relative_path.clone()))];
    conn.execute(stmts.delete_symbols, cleanup_params)?;

    if let (Some(analysis), Some(content)) = (payload.analysis, payload.content) {
        for node in &analysis.nodes {
            let mut raw_calls_str = "[]".to_string();
            let symbol_calls: Vec<crate::types::ast::RawCall> = analysis
                .calls
                .iter()
                .filter(|c| c.line >= node.start_line && c.line <= node.end_line)
                .cloned()
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

pub fn run_index(path: PathBuf, db_path: PathBuf, verbose: bool) {
    println!("==================================================");
    println!("⚡ Synapse Indexer Initializing");
    println!("==================================================");
    println!("Workspace Target : {}", path.display());
    println!("Database Target  : {}", db_path.display());
    println!("--------------------------------------------------");

    if !path.exists() {
        eprintln!(
            "Error: Target workspace path '{}' does not exist.",
            path.display()
        );
        std::process::exit(1);
    }

    if let Some(parent) = db_path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            if let Err(err) = std::fs::create_dir_all(parent) {
                eprintln!("Error: Failed to create database path directory: {}", err);
                std::process::exit(1);
            }
        }
    }

    println!("📦 Connecting to LadybugDB...");
    let db = match Database::new(&db_path, SystemConfig::default()) {
        Ok(database) => database,
        Err(err) => {
            eprintln!(
                "Error: Failed to connect to LadybugDB at '{}': {}",
                db_path.display(),
                err
            );
            std::process::exit(1);
        }
    };

    let conn = match Connection::new(&db) {
        Ok(connection) => connection,
        Err(err) => {
            eprintln!("Error: Failed to open database connection: {}", err);
            std::process::exit(1);
        }
    };

    println!("🛠️  Verifying graph database schema...");
    if let Err(err) = schema::init_schema(&conn) {
        eprintln!("Error: Failed to verify schema tables: {}", err);
        std::process::exit(1);
    }

    if conn
        .query("MATCH (f:File) RETURN f.raw_imports LIMIT 1")
        .is_err()
    {
        eprintln!("\n❌ Database Compatibility Error!");
        eprintln!(
            "The database at '{}' is incompatible (missing 'raw_imports' column on 'File').",
            db_path.display()
        );
        eprintln!("Please delete the database file and run the indexer again to recreate it.");
        std::process::exit(1);
    }
    if conn
        .query("MATCH (s:Symbol) RETURN s.raw_calls LIMIT 1")
        .is_err()
    {
        eprintln!("\n❌ Database Compatibility Error!");
        eprintln!(
            "The database at '{}' is incompatible (missing 'raw_calls' column on 'Symbol').",
            db_path.display()
        );
        eprintln!("Please delete the database file and run the indexer again to recreate it.");
        std::process::exit(1);
    }

    let hash_cache = match load_all_hashes(&conn) {
        Ok(cache) => cache,
        Err(err) => {
            eprintln!(
                "Warning: Failed to load hash cache (will re-index all files): {}",
                err
            );
            HashMap::new()
        }
    };

    let (tx, rx) = std::sync::mpsc::sync_channel::<ParsedPayload>(100);

    // Shared writer-health flag. The writer thread sets this to false on
    // any early return (DB open failure, Connection::new failure); the
    // walker factory checks it before each entry and bails via
    // `WalkState::Quit` to avoid the per-file warning spam that would
    // otherwise flood stderr on a large workspace.
    let writer_alive = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let writer_alive_writer = std::sync::Arc::clone(&writer_alive);

    let db_path_clone = db_path.clone();
    let db_writer = std::thread::spawn(move || {
        let db = match Database::new(&db_path_clone, SystemConfig::default()) {
            Ok(d) => d,
            Err(e) => {
                eprintln!(
                    "Error: DB writer failed to open database '{}': {}",
                    db_path_clone.display(),
                    e
                );
                writer_alive_writer.store(false, std::sync::atomic::Ordering::SeqCst);
                return (0u64, 0u64);
            }
        };
        let conn = match Connection::new(&db) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Error: DB writer failed to connect: {}", e);
                writer_alive_writer.store(false, std::sync::atomic::Ordering::SeqCst);
                return (0u64, 0u64);
            }
        };

        let mut prepared_file_upsert = conn
            .prepare(FILE_UPSERT_CYPHER)
            .expect("Bug: file_upsert prepare failed (hardcoded Cypher)");
        let mut prepared_delete_symbols = conn
            .prepare(DELETE_SYMBOLS_CYPHER)
            .expect("Bug: delete_symbols prepare failed (hardcoded Cypher)");
        let mut prepared_symbol_create = conn
            .prepare(SYMBOL_CREATE_CYPHER)
            .expect("Bug: symbol_create prepare failed (hardcoded Cypher)");
        let mut prepared_containment_file = conn
            .prepare(CONTAINMENT_FILE_CYPHER)
            .expect("Bug: containment_file prepare failed (hardcoded Cypher)");
        let mut prepared_containment_symbol = conn
            .prepare(CONTAINMENT_SYMBOL_CYPHER)
            .expect("Bug: containment_symbol prepare failed (hardcoded Cypher)");
        let mut stmts = PreparedStatements {
            file_upsert: &mut prepared_file_upsert,
            delete_symbols: &mut prepared_delete_symbols,
            symbol_create: &mut prepared_symbol_create,
            containment_file: &mut prepared_containment_file,
            containment_symbol: &mut prepared_containment_symbol,
        };

        let mut local_file_count = 0;
        let mut local_byte_count = 0u64;

        while let Ok(payload) = rx.recv() {
            let size = payload.size;
            let path = payload.relative_path.clone();
            match write_payload_to_db(&conn, payload, &mut stmts, verbose) {
                Ok(_) => {
                    local_file_count += 1;
                    local_byte_count += size;
                }
                Err(err) => {
                    eprintln!(
                        "Error: Failed to index file '{}' in database: {}",
                        path, err
                    );
                }
            }
        }
        (local_file_count, local_byte_count)
    });

    let walker = WalkBuilder::new(&path).build_parallel();
    let abs_db_path = db_path
        .canonicalize()
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default().join(&db_path));
    let path_clone = path.clone();

    let skip_count_atomic = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let skip_count_atomic_clone = skip_count_atomic.clone();

    println!("--------------------------------------------------");
    println!("🔍 Traversing workspace & populating graph...");
    println!("--------------------------------------------------");

    walker.run(|| {
        let tx = tx.clone();
        let writer_alive = std::sync::Arc::clone(&writer_alive);
        let hash_cache = &hash_cache;
        let abs_db_path = &abs_db_path;
        let path_clone = &path_clone;
        let skip_count = &skip_count_atomic_clone;

        Box::new(move |entry| {
            // Bail early once the writer thread has given up. Limits the
            // per-file warning spam to at most one message per walker
            // thread (the file it was already processing when the writer
            // died) instead of one per remaining file in the workspace.
            if !writer_alive.load(std::sync::atomic::Ordering::SeqCst) {
                return ignore::WalkState::Quit;
            }
            let entry = match entry {
                Ok(e) => e,
                Err(_) => return ignore::WalkState::Continue,
            };
            let file_path = entry.path();
            if file_path.is_file() {
                let abs_file = file_path
                    .canonicalize()
                    .unwrap_or_else(|_| file_path.to_path_buf());
                let abs_db_str = abs_db_path.to_string_lossy();
                if abs_file == *abs_db_path
                    || abs_file
                        .to_string_lossy()
                        .starts_with(format!("{}.", abs_db_str).as_str())
                {
                    return ignore::WalkState::Continue;
                }

                match crate::processor::process_file(file_path, path_clone, hash_cache) {
                    Ok(Some(payload)) => {
                        if tx.send(payload).is_err() {
                            eprintln!(
                                "Warning: DB writer is no longer accepting payloads; '{}' will not be indexed (check writer errors above).",
                                file_path.display()
                            );
                        }
                    }
                    Ok(None) => {
                        skip_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    }
                    Err(err) => {
                        eprintln!(
                            "Warning: Failed to process '{}': {}",
                            file_path.display(),
                            err
                        );
                    }
                }
            }
            ignore::WalkState::Continue
        })
    });

    drop(tx);

    let (file_count_res, byte_count_res) = db_writer.join().expect("Bug: DB writer thread panic");
    let file_count = file_count_res;
    let byte_count = byte_count_res;
    let skip_count = skip_count_atomic.load(std::sync::atomic::Ordering::SeqCst);

    println!("--------------------------------------------------");
    println!("✅ Workspace traversal complete!");
    println!("Total Files Indexed/Updated    : {}", file_count);
    println!("Total Files Skipped (Unchanged): {}", skip_count);
    println!(
        "Aggregate Data Size Managed    : {:.2} MB",
        (byte_count as f64) / 1024.0 / 1024.0
    );
    println!("==================================================");

    if let Err(err) = crate::linker::run_linker(&conn, verbose) {
        eprintln!("Error: Global linking phase failed: {}", err);
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lbug::{Connection, Database};

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
    fn test_walker_bails_after_writer_death() {
        // Smoke test: a workspace with many files + a DB path whose parent
        // doesn't exist forces Database::new to fail at writer construction.
        // With the early-bail fix, the walker exits via WalkState::Quit
        // after observing the writer_alive flag flip; without the fix, the
        // walker would visit every file and emit a per-file warning.
        //
        // This test only asserts the run completes; capturing stderr to
        // count warnings would require plumbing a pluggable writer through
        // eprintln!, which is out of scope here.
        let workspace = tempfile::tempdir().unwrap();
        for i in 0..50 {
            std::fs::write(
                workspace.path().join(format!("f_{}.rs", i)),
                "pub fn g() {}\n",
            )
            .unwrap();
        }
        let bad_db = workspace
            .path()
            .join("definitely_does_not_exist")
            .join("foo.lbug");

        run_index(workspace.path().to_path_buf(), bad_db, false);
    }
}
