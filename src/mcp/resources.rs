//! MCP resource handlers: `synapse://repos` and `synapse://repo/{name}/status`.
//!
//! Resources are read-only views over the on-disk registry (`~/.synapse/repos.json`).
//! They power discovery (which repos does the MCP server know about?) and
//! staleness hints (is the index up to date with HEAD?).

use std::path::Path;
use std::process::Command;

use lbug::{Connection, Database, SystemConfig};
use serde_json::{json, Value};

use crate::mcp::repo_registry::{is_stale as entry_is_stale, RepoRegistry};
use crate::mcp::tools::McpToolError;

/// Open a fresh read-only connection to the repo's DB and run `f(conn)`.
fn with_conn_for_db<F, R>(db_path: &Path, f: F) -> Result<R, McpToolError>
where
    F: FnOnce(&Connection) -> Result<R, McpToolError>,
{
    let db = Database::new(db_path, SystemConfig::default())
        .map_err(|e| McpToolError::Internal(format!("open db '{}': {e}", db_path.display())))?;
    let conn = Connection::new(&db)
        .map_err(|e| McpToolError::Internal(format!("open conn: {e}")))?;
    f(&conn)
}

/// Read the `synapse://repos` resource: every registry entry enriched with
/// `is_stale` (via `git rev-parse HEAD` comparison). Empty registry returns `[]`.
pub fn read_repos() -> Result<Value, McpToolError> {
    let registry = RepoRegistry::load().unwrap_or_default();
    let arr: Vec<Value> = registry
        .entries
        .iter()
        .map(|entry| {
            json!({
                "name": entry.name,
                "path": entry.path,
                "db_path": entry.db_path,
                "indexed_at": entry.indexed_at,
                "indexed_commit": entry.indexed_commit,
                "is_stale": entry_is_stale(entry),
            })
        })
        .collect();
    Ok(Value::Array(arr))
}

/// Read the `synapse://repo/{name}/status` resource: single-entry detail view.
///
/// Returns an enriched status object: indexed_at, indexed_commit, current
/// HEAD commit, is_stale, files_count, symbols_count. If `name` is not in
/// the registry, returns `McpToolError::NotFound`.
pub fn read_repo_status(name: &str) -> Result<Value, McpToolError> {
    let registry = RepoRegistry::load().unwrap_or_default();
    let entry = registry
        .find_by_name(name)
        .ok_or_else(|| McpToolError::NotFound(format!("repo '{name}' not found in registry")))?;

    let head_commit = git_head(&entry.path).unwrap_or_default();
    let is_stale = entry_is_stale(entry)
        || (!entry.indexed_commit.is_empty() && head_commit != entry.indexed_commit);

    let (files_count, symbols_count) = if entry.db_path.exists() {
        with_conn_for_db(&entry.db_path, |conn| {
            let files = count_query(conn, "MATCH (f:File) RETURN count(f) AS n")?;
            let symbols = count_query(conn, "MATCH (s:Symbol) RETURN count(s) AS n")?;
            Ok((files, symbols))
        })
        .unwrap_or((0, 0))
    } else {
        (0, 0)
    };

    Ok(json!({
        "name": entry.name,
        "path": entry.path,
        "db_path": entry.db_path,
        "indexed_at": entry.indexed_at,
        "indexed_commit": entry.indexed_commit,
        "head_commit": head_commit,
        "is_stale": is_stale,
        "files_count": files_count,
        "symbols_count": symbols_count,
    }))
}

/// Run a count query and return the integer result. Returns 0 on any error.
fn count_query(conn: &Connection, query: &str) -> Result<usize, McpToolError> {
    let mut result = conn
        .query(query)
        .map_err(|e| McpToolError::Internal(format!("count query: {e}")))?;
    let Some(row) = result.next() else {
        return Ok(0);
    };
    let Some(val) = row.first() else {
        return Ok(0);
    };
    // LadybugDB returns count() as INT64.
    let s = val.to_string();
    Ok(s.parse::<usize>().unwrap_or(0))
}

/// Run `git -C <path> rev-parse HEAD` and return the trimmed stdout. Returns
/// `None` on any failure (non-git path, git not installed, etc.) — never errors.
pub fn git_head(path: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
#[allow(clippy::expect_used)] // test fixtures assert on Option/Result values
mod tests {
    use super::*;

    #[test]
    fn read_repos_returns_array() {
        // Asserts the function shape — actual array contents depend on the
        // user's real `$HOME/.synapse/repos.json` which is empty in CI.
        let result = read_repos();
        assert!(result.is_ok());
        assert!(result.unwrap().is_array());
    }

    #[test]
    fn read_repo_status_returns_not_found_for_unknown_name() {
        let result = read_repo_status("definitely-not-a-real-repo-12345");
        assert!(matches!(result, Err(McpToolError::NotFound(_))));
    }

    #[test]
    fn git_head_returns_none_for_non_git_path() {
        assert!(git_head(Path::new("/tmp")).is_none());
    }

    #[test]
    fn git_head_returns_none_for_nonexistent_path() {
        assert!(git_head(Path::new("/nonexistent/path/that/does/not/exist")).is_none());
    }
}
