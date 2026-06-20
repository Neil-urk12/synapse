// CLI command — stdout is the output. Migrating to `tracing` is tracked
// in `docs/audits/2026-06-15-rust-best-practices-audit.md` Finding 12.
#![allow(clippy::print_stdout)]

use lbug::{Connection, Database, SystemConfig};
use std::path::Path;

/// Failure policy for the post-writer pipeline.
///
/// `FailFast` propagates database-open and linker errors as `Err` —
/// appropriate for one-shot commands (`synapse index`) where a linker
/// failure leaves the graph in a broken state and the user should re-run.
///
/// `Resilient` only warns (and silently skips when the fresh database
/// cannot be opened) — appropriate for the long-running watcher
/// (`synapse watch`), which should keep going across batches even if a
/// single batch's linker pass fails.
///
/// PageRank failure is a warning under both modes — neither call site
/// needs PageRank to be correct to make forward progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostIndexMode {
    FailFast,
    Resilient,
}

/// Run the post-writer pipeline (link, then score) against a fresh
/// database connection.
///
/// The fresh connection is mandatory, not optional: `lbug 0.17`
/// `Connection`s do not see commits made on other connections, and the
/// main-thread connection used by the indexer / watcher was opened
/// before the writer thread started. The "open fresh, run phases, drop"
/// shape concentrates the snapshot workaround in one place so both
/// `run_index` and `run_watch` can stay one-liner callers.
///
/// Returns `Ok(())` once the pipeline completes; per-phase failures are
/// either propagated (FailFast) or warned (Resilient) inside this
/// function. Only an outright inability to open the database under
/// `Resilient` mode is silently skipped — that mirrors `watch.rs`'s
/// pre-existing behavior.
pub fn run(
    db_path: &Path,
    verbose: bool,
    mode: PostIndexMode,
) -> Result<(), Box<dyn std::error::Error>> {
    let fresh_db = match Database::new(db_path, SystemConfig::default()) {
        Ok(db) => db,
        Err(err) => match mode {
            PostIndexMode::FailFast => {
                eprintln!("Error: Could not open fresh DB: {}", err);
                return Err(Box::new(err));
            }
            PostIndexMode::Resilient => {
                // Matches `watch.rs` pre-refactor: the watcher just
                // defers the post-writer phase to the next batch.
                return Ok(());
            }
        },
    };

    let fresh_conn = match Connection::new(&fresh_db) {
        Ok(conn) => conn,
        Err(err) => match mode {
            PostIndexMode::FailFast => {
                eprintln!("Error: Could not open fresh DB connection: {}", err);
                return Err(Box::new(err));
            }
            PostIndexMode::Resilient => return Ok(()),
        },
    };

    if let Err(err) = crate::linker::run_linker(&fresh_conn, verbose) {
        match mode {
            PostIndexMode::FailFast => {
                eprintln!("Error: Global linking phase failed: {}", err);
                return Err(err);
            }
            PostIndexMode::Resilient => {
                eprintln!("Warning: Linker error: {}", err);
            }
        }
    }

    if let Err(err) = crate::pagerank::compute_and_store_pagerank(&fresh_conn, verbose) {
        eprintln!("Warning: PageRank computation failed: {}", err);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal valid database (schema + one File node) for tests
    /// that exercise the success path. Tests that need a corrupt
    /// database skip this and use a freshly created (empty) file.
    fn open_test_db(db_path: &Path) {
        let db = Database::new(db_path, SystemConfig::default()).unwrap();
        let conn = Connection::new(&db).unwrap();
        crate::schema::init_schema(&conn).unwrap();
        // Insert one File node so the linker has something to read.
        let mut stmt = conn
            .prepare(
                "CREATE (f:File {path: $path, language: 'Rust', file_size: 100, \
                 hash: 'h', raw_imports: '[]'})",
            )
            .unwrap();
        conn.execute(
            &mut stmt,
            vec![("path", lbug::Value::String("src/main.rs".to_string()))],
        )
        .unwrap();
    }

    #[test]
    fn test_run_succeeds_on_valid_db_in_failfast_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("ok.lbug");
        open_test_db(&db_path);

        let result = run(&db_path, false, PostIndexMode::FailFast);
        assert!(result.is_ok(), "expected Ok, got {:?}", result.err());
    }

    #[test]
    fn test_run_succeeds_on_valid_db_in_resilient_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("ok.lbug");
        open_test_db(&db_path);

        let result = run(&db_path, false, PostIndexMode::Resilient);
        assert!(result.is_ok(), "expected Ok, got {:?}", result.err());
    }

    /// `FailFast` propagates database-open failure as `Err`. Point the
    /// path at a directory whose parent does not exist; `lbug` will
    /// refuse to open it. This mirrors the regression that used to live
    /// inline in `index.rs` lines 466–487.
    #[test]
    fn test_run_failfast_returns_err_when_db_cannot_be_opened() {
        let tmp = tempfile::tempdir().unwrap();
        let bad_path = tmp.path().join("no_such_dir").join("foo.lbug");

        let result = run(&bad_path, false, PostIndexMode::FailFast);
        assert!(
            result.is_err(),
            "FailFast must propagate DB-open failure as Err"
        );
    }

    /// `Resilient` silently skips when the fresh database cannot be
    /// opened — the watcher will retry on the next batch. This mirrors
    /// `watch.rs` lines 195–206, which used `if let Ok(...)` with no
    /// else arm.
    #[test]
    fn test_run_resilient_returns_ok_when_db_cannot_be_opened() {
        let tmp = tempfile::tempdir().unwrap();
        let bad_path = tmp.path().join("no_such_dir").join("foo.lbug");

        let result = run(&bad_path, false, PostIndexMode::Resilient);
        assert!(
            result.is_ok(),
            "Resilient must silently skip DB-open failure, got {:?}",
            result.err()
        );
    }

    /// `FailFast` propagates linker failure as `Err`. Use an empty,
    /// freshly-created database (no schema, no nodes) so the linker's
    /// `MATCH (f:File)` query fails.
    #[test]
    fn test_run_failfast_returns_err_on_linker_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("empty.lbug");

        // Create the file but leave it empty — the linker's MATCH query
        // against a missing File table will error.
        std::fs::write(&db_path, b"").unwrap();

        let result = run(&db_path, false, PostIndexMode::FailFast);
        assert!(
            result.is_err(),
            "FailFast must propagate linker failure as Err"
        );
    }

    /// `Resilient` only warns on linker failure. Same broken DB, but the
    /// watcher should keep going.
    #[test]
    fn test_run_resilient_returns_ok_on_linker_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("empty.lbug");
        std::fs::write(&db_path, b"").unwrap();

        let result = run(&db_path, false, PostIndexMode::Resilient);
        assert!(
            result.is_ok(),
            "Resilient must swallow linker failure (with warning), got {:?}",
            result.err()
        );
    }
}
