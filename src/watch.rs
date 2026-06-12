use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use lbug::{Connection, Database, SystemConfig, Value};
use notify::Watcher;

pub fn run_watch(path: &Path, db_path: &Path, debounce_secs: u64, verbose: bool) {
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

    println!("⚡ Synapse Watcher Starting — full initial index...");

    // Phase 1: Full initial index (run_index opens its own DB internally,
    // and calls run_linker internally)
    crate::index::run_index(path.to_path_buf(), db_path.to_path_buf(), verbose);

    // Phase 2: Open persistent connection for watch loop
    let db = match Database::new(db_path, SystemConfig::default()) {
        Ok(database) => database,
        Err(err) => {
            eprintln!(
                "Error: Failed to open database at '{}': {}",
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

    if let Err(err) = crate::schema::init_schema(&conn) {
        eprintln!("Error: Failed to verify schema tables: {}", err);
        std::process::exit(1);
    }

    println!("✅ Initial index complete. Watching for changes...");

    // Phase 3: Watcher setup
    let (tx, rx) = std::sync::mpsc::channel();

    let mut watcher = match notify::recommended_watcher(tx) {
        Ok(w) => w,
        Err(err) => {
            eprintln!("Error: Failed to create file watcher: {}", err);
            std::process::exit(1);
        }
    };

    if let Err(err) = watcher.watch(path, notify::RecursiveMode::Recursive) {
        eprintln!("Error: Failed to watch path '{}': {}", path.display(), err);
        std::process::exit(1);
    }

    // Phase 4: Event loop
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    })
    .expect("Bug: Failed to set Ctrl-C handler");

    let workspace_root = path
        .canonicalize()
        .unwrap_or_else(|_| path.to_path_buf());
    let db_name = db_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("synapse.lbug")
        .to_string();
    let debounce_dur = Duration::from_secs(debounce_secs);

    let mut pending: HashSet<PathBuf> = HashSet::new();
    let mut batch_count = 0u64;

    while running.load(Ordering::SeqCst) {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(event) => {
                let e = match event {
                    Ok(e) => e,
                    Err(err) => {
                        eprintln!("Warning: File watcher error: {}", err);
                        continue;
                    }
                };
                // Skip access events (metadata reads, atime updates)
                if e.kind.is_access() {
                    continue;
                }
                for p in &e.paths {
                    if should_watch(p, &db_name) {
                        pending.insert(p.clone());
                    }
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if pending.is_empty() {
                    continue;
                }
                // Wait for debounce period — collect more events
                let wait_until = std::time::Instant::now() + debounce_dur;
                loop {
                    let remaining = wait_until.saturating_duration_since(std::time::Instant::now());
                    if remaining.is_zero() {
                        break;
                    }
                    match rx.recv_timeout(remaining) {
                        Ok(Ok(e)) => {
                            if !e.kind.is_access() {
                                for p in &e.paths {
                                    if should_watch(p, &db_name) {
                                        pending.insert(p.clone());
                                    }
                                }
                            }
                        }
                        Ok(Err(err)) => {
                            eprintln!("Warning: File watcher error: {}", err);
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }

                batch_count += 1;
                if verbose {
                    println!(
                        "  [batch #{}] Processing {} changed files...",
                        batch_count,
                        pending.len()
                    );
                }
                if let Err(err) =
                    process_batch(&conn, &workspace_root, &pending, verbose)
                {
                    eprintln!("Warning: Batch processing error: {}", err);
                }
                if verbose {
                    println!("  [batch #{}] Relinking...", batch_count);
                }
                if let Err(err) = crate::linker::run_linker(&conn, verbose) {
                    eprintln!("Warning: Linker error: {}", err);
                }
                if !verbose {
                    println!(
                        "  Updated {} files (batch #{})",
                        pending.len(),
                        batch_count
                    );
                }
                pending.clear();
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    println!("👋 Shutting down. Processed {} batches.", batch_count);
}

fn should_watch(path: &Path, db_name: &str) -> bool {
    if !path.is_absolute() {
        return false;
    }
    // Skip DB file and companions (.wal, .tmp, etc.)
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        if name.starts_with(db_name) {
            return false;
        }
    }
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    matches!(
        ext,
        "rs" | "js" | "jsx" | "ts" | "tsx" | "go" | "py" | "c" | "cpp" | "cc" | "cxx" | "h"
            | "hpp" | "java" | "kt" | "kts"
    )
}

fn get_cached_hash(
    conn: &Connection,
    relative_path: &str,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let mut stmt = conn.prepare("MATCH (f:File {path: $path}) RETURN f.hash")?;
    let result = conn.execute(
        &mut stmt,
        vec![("path", Value::String(relative_path.to_string()))],
    )?;
    for row in result {
        if let Some(Value::String(hash)) = row.first() {
            return Ok(Some(hash.clone()));
        }
    }
    Ok(None)
}

fn process_remove(
    conn: &Connection,
    relative_path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut stmt = conn.prepare("MATCH (f:File {path: $path}) DETACH DELETE f")?;
    conn.execute(
        &mut stmt,
        vec![("path", Value::String(relative_path.to_string()))],
    )?;
    Ok(())
}

fn process_batch(
    conn: &Connection,
    workspace_root: &Path,
    paths: &HashSet<PathBuf>,
    verbose: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    for path in paths {
        let relative_path = path
            .strip_prefix(workspace_root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();

        if !path.exists() {
            process_remove(conn, &relative_path)?;
            if verbose {
                println!("  Removed: {}", relative_path);
            }
            continue;
        }

        let hash = match crate::file_utils::compute_sha256(path) {
            Ok(h) => h,
            Err(err) => {
                eprintln!("Warning: Cannot hash '{}': {}", relative_path, err);
                continue;
            }
        };

        // Skip if unchanged
        if let Ok(Some(cached)) = get_cached_hash(conn, &relative_path) {
            if cached == hash {
                if verbose {
                    println!("  Skipped (unchanged): {}", relative_path);
                }
                continue;
            }
        }

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(err) => {
                eprintln!("Warning: Cannot read '{}': {}", relative_path, err);
                continue;
            }
        };

        let lang = crate::file_utils::detect_language(path);
        let analysis = crate::parser::ASTParser::parse_file(path, &content);
        let size = content.len() as u64;

        let payload = crate::types::db::ParsedPayload {
            relative_path: relative_path.clone(),
            language: lang,
            size,
            hash,
            analysis: Some(analysis),
            content: Some(content),
        };

        // Prepare fresh statements for this file
        let mut file_upsert = conn.prepare(
            "MERGE (f:File {path: $path}) \
             ON CREATE SET f.language = $language, f.file_size = $file_size, f.hash = $hash, f.raw_imports = $raw_imports \
             ON MATCH SET f.language = $language, f.file_size = $file_size, f.hash = $hash, f.raw_imports = $raw_imports",
        )?;
        let mut delete_symbols = conn
            .prepare("MATCH (f:File {path: $path})-[:CONTAINS*1..]->(s:Symbol) DETACH DELETE s")?;
        let mut symbol_create = conn.prepare(
            "MERGE (s:Symbol {id: $id}) \
             ON CREATE SET s.name = $name, s.kind = $kind, s.start_line = $start_line, s.start_col = $start_col, s.end_line = $end_line, s.signature = $signature, s.raw_calls = $raw_calls \
             ON MATCH SET s.name = $name, s.kind = $kind, s.start_line = $start_line, s.start_col = $start_col, s.end_line = $end_line, s.signature = $signature, s.raw_calls = $raw_calls",
        )?;
        let mut containment_file = conn.prepare(
            "MATCH (f:File {path: $from_id}), (s:Symbol {id: $to_id}) MERGE (f)-[:CONTAINS]->(s)",
        )?;
        let mut containment_symbol = conn.prepare(
            "MATCH (p:Symbol {id: $from_id}), (c:Symbol {id: $to_id}) MERGE (p)-[:CONTAINS]->(c)",
        )?;

        let mut stmts = crate::index::PreparedStatements {
            file_upsert: &mut file_upsert,
            delete_symbols: &mut delete_symbols,
            symbol_create: &mut symbol_create,
            containment_file: &mut containment_file,
            containment_symbol: &mut containment_symbol,
        };

        if let Err(err) =
            crate::index::write_payload_to_db(conn, payload, &mut stmts, verbose)
        {
            eprintln!("Warning: Failed to index '{}': {}", relative_path, err);
        } else if verbose {
            println!("  Indexed: {}", relative_path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- should_watch tests ---

    #[test]
    fn test_should_watch_supported_extension_rs() {
        let path = Path::new("/tmp/test.rs");
        assert!(should_watch(path, "synapse.lbug"));
    }

    #[test]
    fn test_should_watch_supported_extensions() {
        let exts = [
            "rs", "js", "jsx", "ts", "tsx", "go", "py", "c", "cpp", "h", "hpp", "java", "kt",
            "kts",
        ];
        for ext in &exts {
            let filename = format!("/tmp/test.{}", ext);
            let path = Path::new(&filename);
            assert!(
                should_watch(path, "synapse.lbug"),
                "expected {} to be watched",
                ext
            );
        }
    }

    #[test]
    fn test_should_watch_unsupported_extension() {
        let path = Path::new("/tmp/test.txt");
        assert!(!should_watch(path, "synapse.lbug"));
    }

    #[test]
    fn test_should_watch_no_extension() {
        let path = Path::new("/tmp/Makefile");
        assert!(!should_watch(path, "synapse.lbug"));
    }

    #[test]
    fn test_should_watch_skips_db_file() {
        let path = Path::new("/tmp/synapse.lbug");
        assert!(!should_watch(path, "synapse.lbug"));
    }

    #[test]
    fn test_should_watch_skips_db_companions() {
        let path = Path::new("/tmp/synapse.lbug.wal");
        assert!(!should_watch(path, "synapse.lbug"));
    }

    #[test]
    fn test_should_watch_non_absolute_path() {
        let path = Path::new("test.rs");
        assert!(!should_watch(path, "synapse.lbug"));
    }

    #[test]
    fn test_should_watch_absolute_path_supported_ext() {
        let path = Path::new("/home/user/project/src/main.rs");
        assert!(should_watch(path, "synapse.lbug"));
    }

    // --- get_cached_hash tests ---

    #[test]
    fn test_get_cached_hash_empty_db_returns_none() {
        let db_path = Path::new("test_get_cached_hash_empty_unique.lbug");
        if db_path.exists() {
            let _ = std::fs::remove_dir_all(db_path);
        }
        let db = lbug::Database::new(db_path, lbug::SystemConfig::default()).unwrap();
        let conn = lbug::Connection::new(&db).unwrap();
        crate::schema::init_schema(&conn).unwrap();

        let result = get_cached_hash(&conn, "nonexistent.rs").unwrap();
        assert!(result.is_none());

        drop(conn);
        drop(db);
        if db_path.exists() {
            let _ = std::fs::remove_dir_all(db_path);
        }
    }

    #[test]
    fn test_get_cached_hash_finds_existing_hash() {
        let db_path = Path::new("test_get_cached_hash_finds_unique.lbug");
        if db_path.exists() {
            let _ = std::fs::remove_dir_all(db_path);
        }
        let db = lbug::Database::new(db_path, lbug::SystemConfig::default()).unwrap();
        let conn = lbug::Connection::new(&db).unwrap();
        crate::schema::init_schema(&conn).unwrap();

        // Insert a File node
        let mut stmt = conn
            .prepare(
                "MERGE (f:File {path: $path}) \
             ON CREATE SET f.language = 'Rust', f.file_size = 100, f.hash = $hash, f.raw_imports = '[]'",
            )
            .unwrap();
        conn.execute(
            &mut stmt,
            vec![
                ("path", Value::String("src/main.rs".to_string())),
                ("hash", Value::String("abc123".to_string())),
            ],
        )
        .unwrap();

        let result = get_cached_hash(&conn, "src/main.rs").unwrap();
        assert_eq!(result, Some("abc123".to_string()));

        drop(conn);
        drop(db);
        if db_path.exists() {
            let _ = std::fs::remove_dir_all(db_path);
        }
    }

    // --- process_remove tests ---

    #[test]
    fn test_process_remove_removes_existing_file() {
        let db_path = Path::new("test_process_remove_existing_unique.lbug");
        if db_path.exists() {
            let _ = std::fs::remove_dir_all(db_path);
        }
        let db = lbug::Database::new(db_path, lbug::SystemConfig::default()).unwrap();
        let conn = lbug::Connection::new(&db).unwrap();
        crate::schema::init_schema(&conn).unwrap();

        // Insert a File node
        let mut stmt = conn
            .prepare(
                "MERGE (f:File {path: $path}) \
             ON CREATE SET f.language = 'Rust', f.file_size = 100, f.hash = 'abc', f.raw_imports = '[]'",
            )
            .unwrap();
        conn.execute(
            &mut stmt,
            vec![("path", Value::String("gone.rs".to_string()))],
        )
        .unwrap();

        // Verify it exists
        assert!(get_cached_hash(&conn, "gone.rs").unwrap().is_some());

        // Remove
        process_remove(&conn, "gone.rs").unwrap();

        // Verify it's gone
        assert!(get_cached_hash(&conn, "gone.rs").unwrap().is_none());

        drop(conn);
        drop(db);
        if db_path.exists() {
            let _ = std::fs::remove_dir_all(db_path);
        }
    }

    #[test]
    fn test_process_remove_non_existent_is_noop() {
        let db_path = Path::new("test_process_remove_noop_unique.lbug");
        if db_path.exists() {
            let _ = std::fs::remove_dir_all(db_path);
        }
        let db = lbug::Database::new(db_path, lbug::SystemConfig::default()).unwrap();
        let conn = lbug::Connection::new(&db).unwrap();
        crate::schema::init_schema(&conn).unwrap();

        // Removing non-existent file should not error
        process_remove(&conn, "nonexistent.rs").unwrap();

        drop(conn);
        drop(db);
        if db_path.exists() {
            let _ = std::fs::remove_dir_all(db_path);
        }
    }

    // --- process_batch tests ---

    #[test]
    fn test_process_batch_indexes_new_rs_file() {
        let db_name = "test_process_batch_new_rs_unique";
        let db_filename = format!("{}.lbug", db_name);
        let db_path = Path::new(&db_filename);
        if db_path.exists() {
            let _ = std::fs::remove_dir_all(db_path);
        }

        // Create temp workspace with a rust file
        let dir = std::env::temp_dir().join(db_name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file_path = dir.join("hello.rs");
        std::fs::write(&file_path, b"fn greet() -> &str { \"hi\" }").unwrap();

        // Setup DB
        let db = lbug::Database::new(db_path, lbug::SystemConfig::default()).unwrap();
        let conn = lbug::Connection::new(&db).unwrap();
        crate::schema::init_schema(&conn).unwrap();

        let mut paths = HashSet::new();
        paths.insert(file_path.clone());

        let result = process_batch(&conn, &dir, &paths, false);
        assert!(
            result.is_ok(),
            "process_batch failed: {:?}",
            result.err()
        );

        // Verify file was indexed
        let hash = get_cached_hash(&conn, "hello.rs").unwrap();
        assert!(hash.is_some(), "file should have been indexed");

        // Cleanup
        drop(conn);
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
        if db_path.exists() {
            let _ = std::fs::remove_dir_all(db_path);
        }
    }

    #[test]
    fn test_process_batch_skips_unchanged_file() {
        let db_name = "test_process_batch_skip_unchanged_unique";
        let db_filename = format!("{}.lbug", db_name);
        let db_path = Path::new(&db_filename);
        if db_path.exists() {
            let _ = std::fs::remove_dir_all(db_path);
        }

        // Create temp workspace
        let dir = std::env::temp_dir().join(db_name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file_path = dir.join("main.rs");
        std::fs::write(&file_path, b"fn main() {}").unwrap();

        // Setup DB
        let db = lbug::Database::new(db_path, lbug::SystemConfig::default()).unwrap();
        let conn = lbug::Connection::new(&db).unwrap();
        crate::schema::init_schema(&conn).unwrap();

        // First pass -- index the file
        let mut paths = HashSet::new();
        paths.insert(file_path.clone());
        process_batch(&conn, &dir, &paths, false).unwrap();

        // Record initial hash
        let hash_after_first = get_cached_hash(&conn, "main.rs").unwrap().unwrap();

        // Second pass -- file hasn't changed
        let mut paths2 = HashSet::new();
        paths2.insert(file_path.clone());
        process_batch(&conn, &dir, &paths2, false).unwrap();

        // Hash should be same (file not re-processed)
        let hash_after_second = get_cached_hash(&conn, "main.rs").unwrap().unwrap();
        assert_eq!(hash_after_first, hash_after_second);

        // Cleanup
        drop(conn);
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
        if db_path.exists() {
            let _ = std::fs::remove_dir_all(db_path);
        }
    }

    #[test]
    fn test_process_batch_handles_deleted_file() {
        let db_name = "test_process_batch_deleted_unique";
        let db_filename = format!("{}.lbug", db_name);
        let db_path = Path::new(&db_filename);
        if db_path.exists() {
            let _ = std::fs::remove_dir_all(db_path);
        }

        // Create temp workspace
        let dir = std::env::temp_dir().join(db_name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file_path = dir.join("temp.rs");
        std::fs::write(&file_path, b"fn temp() {}").unwrap();

        // Setup DB
        let db = lbug::Database::new(db_path, lbug::SystemConfig::default()).unwrap();
        let conn = lbug::Connection::new(&db).unwrap();
        crate::schema::init_schema(&conn).unwrap();

        // Index it
        let mut paths = HashSet::new();
        paths.insert(file_path.clone());
        process_batch(&conn, &dir, &paths, false).unwrap();
        assert!(get_cached_hash(&conn, "temp.rs").unwrap().is_some());

        // Delete file and re-run batch
        std::fs::remove_file(&file_path).unwrap();
        let mut paths2 = HashSet::new();
        paths2.insert(file_path.clone());
        process_batch(&conn, &dir, &paths2, false).unwrap();

        // File should be removed from DB
        assert!(get_cached_hash(&conn, "temp.rs").unwrap().is_none());

        // Cleanup
        drop(conn);
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
        if db_path.exists() {
            let _ = std::fs::remove_dir_all(db_path);
        }
    }

    #[test]
    fn test_process_batch_re_indexes_changed_file() {
        let db_name = "test_process_batch_reindex_unique";
        let db_filename = format!("{}.lbug", db_name);
        let db_path = Path::new(&db_filename);
        if db_path.exists() {
            let _ = std::fs::remove_dir_all(db_path);
        }

        // Create temp workspace
        let dir = std::env::temp_dir().join(db_name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file_path = dir.join("lib.rs");
        std::fs::write(&file_path, b"fn old() {}").unwrap();

        // Setup DB
        let db = lbug::Database::new(db_path, lbug::SystemConfig::default()).unwrap();
        let conn = lbug::Connection::new(&db).unwrap();
        crate::schema::init_schema(&conn).unwrap();

        // Index original
        let mut paths = HashSet::new();
        paths.insert(file_path.clone());
        process_batch(&conn, &dir, &paths, false).unwrap();
        let hash_old = get_cached_hash(&conn, "lib.rs").unwrap().unwrap();

        // Modify file
        std::fs::write(&file_path, b"fn new() -> i32 { 42 }").unwrap();

        // Re-index
        let mut paths2 = HashSet::new();
        paths2.insert(file_path.clone());
        process_batch(&conn, &dir, &paths2, false).unwrap();
        let hash_new = get_cached_hash(&conn, "lib.rs").unwrap().unwrap();

        // Hash should have changed
        assert_ne!(hash_old, hash_new);

        // Cleanup
        drop(conn);
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
        if db_path.exists() {
            let _ = std::fs::remove_dir_all(db_path);
        }
    }
}
