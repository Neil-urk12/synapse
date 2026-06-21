//! MCP tool implementations (rmcp-agnostic thin wrappers over query handlers).
//!
//! Each tool opens a fresh LadybugDB Connection per invocation (matches the
//! project's existing pattern; `Connection` is `!Send` and borrows from
//! `Database`, so we wrap work in a closure to keep both alive). Tools that
//! already produce JSON via the existing handler pipeline (`run_call_graph`,
//! `run_dependencies`, `run_context`) capture the formatted output into a
//! `Vec<u8>` and parse it. Tools without a JSON output path
//! (`synapse_query`, `synapse_rank`, `synapse_cypher`) call `conn.query()`
//! directly and serialize the `QueryResult` themselves.

use std::path::{Path, PathBuf};

use lbug::{Connection, Database, SystemConfig};
use serde_json::{json, Value};

use crate::mcp::repo_registry::{RegistryError, RepoRegistry};
use crate::mcp::tools::{McpTool, McpToolError};

// ─── Shared helpers ─────────────────────────────────────────────────────

/// Open a fresh Database + Connection at `db_path` and pass the connection
/// to `f`. Mirrors the `with_db` helper in `main.rs` but returns `Result<R, McpToolError>`
/// directly. `Connection` borrows from `Database`, so the closure pattern
/// keeps both alive for the duration of the work.
fn with_conn<F, R>(db_path: &Path, f: F) -> Result<R, McpToolError>
where
    F: FnOnce(&Connection) -> Result<R, McpToolError>,
{
    let db = Database::new(db_path, SystemConfig::default())
        .map_err(|e| McpToolError::Internal(format!("open db '{}': {e}", db_path.display())))?;
    let conn =
        Connection::new(&db).map_err(|e| McpToolError::Internal(format!("open conn: {e}")))?;
    f(&conn)
}

/// Resolve a DB path from the tool's `repo` argument.
///
/// If `repo` is a non-empty string, look it up in `~/.synapse/repos.json`
/// and return the entry's `db_path`. An empty string is treated the same
/// as a missing `repo` argument. If the argument is missing/empty, fall
/// back to `./synapse.lbug` in the current working directory if it exists.
fn resolve_db_path(repo: Option<&Value>) -> Result<PathBuf, McpToolError> {
    let name = match repo.and_then(|v| v.as_str()) {
        Some(s) if !s.is_empty() => s,
        _ => return resolve_cwd_db(),
    };

    let registry = RepoRegistry::load().map_err(|e| match e {
        RegistryError::NoHome => {
            McpToolError::Internal("HOME is unset; cannot read ~/.synapse/repos.json".into())
        }
        other => McpToolError::Internal(format!("registry load failed: {other}")),
    })?;

    let entry = registry.find_by_name(name).ok_or_else(|| {
        McpToolError::NotFound(format!(
            "repo '{name}' not found in registry; see synapse://repos for available repos"
        ))
    })?;

    if !entry.db_path.exists() {
        return Err(McpToolError::InvalidParams(format!(
            "db file '{}' not found for repo '{name}' (run `synapse index` to re-create)",
            entry.db_path.display()
        )));
    }

    Ok(entry.db_path.clone())
}

fn resolve_cwd_db() -> Result<PathBuf, McpToolError> {
    let cwd_db = std::env::current_dir()
        .map_err(|e| McpToolError::Internal(format!("cwd unavailable: {e}")))?
        .join("synapse.lbug");
    if cwd_db.exists() {
        Ok(cwd_db)
    } else {
        Err(McpToolError::InvalidParams(
            "no repo specified and no synapse.lbug in cwd; \
             see synapse://repos for available repos"
                .to_string(),
        ))
    }
}

fn require_string<'a>(args: &'a Value, key: &str) -> Result<&'a str, McpToolError> {
    args.get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpToolError::InvalidParams(format!("missing '{key}' string")))
}

fn require_u64(args: &Value, key: &str) -> Option<u64> {
    args.get(key).and_then(|v| v.as_u64())
}

/// Classify `Box<dyn Error>` from existing query handlers into McpToolError.
/// String-based for v1; a future change can introduce typed downcasting once
/// the handlers return `Result<_, CandidateError>` directly.
fn classify_query_error(e: Box<dyn std::error::Error>) -> McpToolError {
    let msg = e.to_string();
    if msg.contains("matches multiple") || msg.contains("Ambiguous") {
        McpToolError::InvalidParams(msg)
    } else if msg.contains("No symbol matches") || msg.contains("NoMatch") {
        McpToolError::NotFound(msg)
    } else {
        McpToolError::Internal(msg)
    }
}

/// Capture handler output (written via `writeln!` to a `&mut dyn Write`)
/// and parse as JSON. Strips trailing whitespace before parsing.
fn capture_and_parse_json(buf: &[u8]) -> Result<Value, McpToolError> {
    let s = std::str::from_utf8(buf)
        .map_err(|e| McpToolError::Internal(format!("utf8: {e}")))?
        .trim();
    serde_json::from_str(s)
        .map_err(|e| McpToolError::Internal(format!("parse output: {e}; raw={s}")))
}

/// Serialize a `lbug::QueryResult` to a JSON array of row objects, where each
/// row object maps column name → value's `to_string()` (LadybugDB Value's
/// display is the canonical wire representation).
fn query_result_to_json(result: &mut lbug::QueryResult) -> Result<Value, McpToolError> {
    let headers = result.get_column_names();
    let mut rows: Vec<Value> = Vec::new();
    for row in result {
        let obj: serde_json::Map<String, Value> = headers
            .iter()
            .zip(row.iter())
            .map(|(name, val)| (name.clone(), Value::String(val.to_string())))
            .collect();
        rows.push(Value::Object(obj));
    }
    Ok(Value::Array(rows))
}

// ─── Tool implementations ───────────────────────────────────────────────

/// `synapse_cypher`: execute a raw Cypher query against the code graph.
pub struct CypherTool;

#[async_trait::async_trait]
impl McpTool for CypherTool {
    fn name(&self) -> &'static str {
        "synapse_cypher"
    }
    fn description(&self) -> &'static str {
        "Execute a raw Cypher query against the code graph. Returns a JSON array of result rows."
    }
    async fn invoke(&self, args: Value) -> Result<Value, McpToolError> {
        let query = require_string(&args, "query")?.to_string();
        let db_path = resolve_db_path(args.get("repo"))?;
        with_conn(&db_path, |conn| {
            let mut result = conn
                .query(&query)
                .map_err(|e| McpToolError::InvalidParams(format!("query: {e}")))?;
            query_result_to_json(&mut result)
        })
    }
}

/// `synapse_callers`: find direct and transitive callers of a symbol.
pub struct CallersTool;

#[async_trait::async_trait]
impl McpTool for CallersTool {
    fn name(&self) -> &'static str {
        "synapse_callers"
    }
    fn description(&self) -> &'static str {
        "Find direct and transitive callers of a target symbol. Walks the full CALLS graph."
    }
    async fn invoke(&self, args: Value) -> Result<Value, McpToolError> {
        let name = require_string(&args, "name")?.to_string();
        // `depth` and `top` are accepted for spec compliance but ignored for
        // v1: `run_call_graph` walks the full graph with no row cap. A future
        // change plumbs these through to the handler.
        let _ = require_u64(&args, "depth");
        let _ = require_u64(&args, "top");

        let db_path = resolve_db_path(args.get("repo"))?;
        with_conn(&db_path, |conn| {
            let mut buf: Vec<u8> = Vec::new();
            crate::query::handler::run_call_graph(
                conn,
                &name,
                false,
                crate::query::handler::Direction::Callers,
                crate::query::format::QueryFormat::Json,
                &mut buf,
            )
            .map_err(classify_query_error)?;
            capture_and_parse_json(&buf)
        })
    }
}

/// `synapse_callees`: find what a target symbol calls.
pub struct CalleesTool;

#[async_trait::async_trait]
impl McpTool for CalleesTool {
    fn name(&self) -> &'static str {
        "synapse_callees"
    }
    fn description(&self) -> &'static str {
        "Find what a target symbol calls. Walks one level (callees are direct dependencies)."
    }
    async fn invoke(&self, args: Value) -> Result<Value, McpToolError> {
        let name = require_string(&args, "name")?.to_string();
        let _ = require_u64(&args, "top");

        let db_path = resolve_db_path(args.get("repo"))?;
        with_conn(&db_path, |conn| {
            let mut buf: Vec<u8> = Vec::new();
            crate::query::handler::run_call_graph(
                conn,
                &name,
                false,
                crate::query::handler::Direction::Callees,
                crate::query::format::QueryFormat::Json,
                &mut buf,
            )
            .map_err(classify_query_error)?;
            capture_and_parse_json(&buf)
        })
    }
}

/// `synapse_deps`: list imports + imported-by for a target file.
pub struct DepsTool;

#[async_trait::async_trait]
impl McpTool for DepsTool {
    fn name(&self) -> &'static str {
        "synapse_deps"
    }
    fn description(&self) -> &'static str {
        "List imports and imported-by for a target file (synapse deps)."
    }
    async fn invoke(&self, args: Value) -> Result<Value, McpToolError> {
        let file = require_string(&args, "file")?.to_string();
        let db_path = resolve_db_path(args.get("repo"))?;
        with_conn(&db_path, |conn| {
            let mut buf: Vec<u8> = Vec::new();
            crate::query::handler::run_dependencies(
                conn,
                &file,
                false,
                crate::query::format::QueryFormat::Json,
                &mut buf,
            )
            .map_err(classify_query_error)?;
            capture_and_parse_json(&buf)
        })
    }
}

/// `synapse_context`: retrieve consolidated code intelligence for a symbol or file.
pub struct ContextTool;

#[async_trait::async_trait]
impl McpTool for ContextTool {
    fn name(&self) -> &'static str {
        "synapse_context"
    }
    fn description(&self) -> &'static str {
        "Retrieve code intelligence for a symbol or file: signature, source slice, call graph, and dependencies."
    }
    async fn invoke(&self, args: Value) -> Result<Value, McpToolError> {
        let name = args.get("name").and_then(|v| v.as_str());
        let file = args.get("file").and_then(|v| v.as_str());
        if name.is_none() && file.is_none() {
            return Err(McpToolError::InvalidParams(
                "either 'name' or 'file' is required".into(),
            ));
        }
        if name.is_some() && file.is_some() {
            return Err(McpToolError::InvalidParams(
                "'name' and 'file' are mutually exclusive".into(),
            ));
        }
        let db_path = resolve_db_path(args.get("repo"))?;
        with_conn(&db_path, |conn| {
            let mut buf: Vec<u8> = Vec::new();
            crate::query::handler::run_context(
                conn,
                name,
                file,
                true, // fuzzy match
                crate::query::format::QueryFormat::parse_for_context("json")
                    .map_err(|e| McpToolError::Internal(format!("format: {e}")))?,
                &mut buf,
            )
            .map_err(classify_query_error)?;
            capture_and_parse_json(&buf)
        })
    }
}

/// `synapse_query`: semantic search over indexed code chunks via embeddings.
pub struct QueryTool;

#[allow(clippy::cast_possible_truncation)] // f64 → f32 on threshold arg; range 0.0–1.0 in practice
#[async_trait::async_trait]
impl McpTool for QueryTool {
    fn name(&self) -> &'static str {
        "synapse_query"
    }
    fn description(&self) -> &'static str {
        "Semantic search over indexed code chunks. Requires `synapse embed` to have been run."
    }
    async fn invoke(&self, args: Value) -> Result<Value, McpToolError> {
        let query = require_string(&args, "query")?.to_string();
        let limit = require_u64(&args, "limit")
            .map(|n| usize::try_from(n).unwrap_or(usize::MAX))
            .unwrap_or(10);
        let threshold = args
            .get("threshold")
            .and_then(|v| v.as_f64())
            .map(|f| f as f32)
            .unwrap_or(0.5);

        let db_path = resolve_db_path(args.get("repo"))?;
        let mut model = crate::embedder::Embedder::try_new()
            .map_err(|e| McpToolError::Internal(format!("load embedder: {e}")))?;
        let query_emb = model
            .embed(std::slice::from_ref(&query))
            .map_err(|e| McpToolError::Internal(format!("embed: {e}")))?;
        let query_vec = &query_emb[0];
        let vec_str: Vec<String> = query_vec.iter().map(|f| f.to_string()).collect();

        with_conn(&db_path, |conn| {
            let search_query = format!(
                "CALL QUERY_VECTOR_INDEX('Chunk', 'idx_chunk_vector', [{}], {}) \
                 YIELD node, distance RETURN node.id, node.text, node.language, distance",
                vec_str.join(", "),
                limit
            );
            let result = conn
                .query(&search_query)
                .map_err(|e| McpToolError::Internal(format!("vector query: {e}")))?;
            let scored = crate::similar::scored_chunks_from_result(result, threshold);
            if scored.is_empty() {
                // Distinguish "no embeddings exist" from "no matches above threshold".
                // Cheap heuristic: count chunks with embeddings. If zero, the
                // index was never embedded.
                let count_res = conn
                    .query("MATCH (c:Chunk) WHERE c.embedding IS NOT NULL RETURN count(c) AS n");
                let any_embedded = count_res
                    .ok()
                    .and_then(|mut r| r.next().and_then(|row| row.first().cloned()))
                    .map(|v| v.to_string() != "0")
                    .unwrap_or(false);
                if !any_embedded {
                    return Err(McpToolError::InvalidParams(
                        "run 'synapse embed' to populate embeddings".into(),
                    ));
                }
                return Ok(Value::Array(vec![]));
            }
            let arr: Vec<Value> = scored
                .iter()
                .map(|s| {
                    json!({
                        "chunk_id": s.id,
                        "score": s.score,
                        "language": s.language,
                        "source_code": s.text,
                    })
                })
                .collect();
            Ok(Value::Array(arr))
        })
    }
}

/// `synapse_impact`: blast radius sorted by PageRank.
pub struct ImpactTool;

#[async_trait::async_trait]
impl McpTool for ImpactTool {
    fn name(&self) -> &'static str {
        "synapse_impact"
    }
    fn description(&self) -> &'static str {
        "Show all symbols transitively affected by changes to a target symbol, sorted by PageRank."
    }
    async fn invoke(&self, args: Value) -> Result<Value, McpToolError> {
        let symbol = require_string(&args, "name")?.to_string();
        let top = require_u64(&args, "top").map(|n| usize::try_from(n).unwrap_or(usize::MAX));
        let max_depth =
            require_u64(&args, "max_depth").map(|n| usize::try_from(n).unwrap_or(usize::MAX));

        let db_path = resolve_db_path(args.get("repo"))?;
        with_conn(&db_path, |conn| {
            let opts = crate::impact::ImpactOptions { max_depth, top };
            let rows = crate::impact::run_impact(conn, &symbol, true, &opts).map_err(|e| {
                use crate::impact::ImpactError;
                match e {
                    ImpactError::NoMatch { .. } => McpToolError::NotFound(e.to_string()),
                    ImpactError::Io(_) => McpToolError::Internal(e.to_string()),
                }
            })?;
            let arr: Vec<Value> = rows
                .iter()
                .map(|r| {
                    json!({
                        "rank": r.rank,
                        "symbol": r.id,
                        "kind": r.kind,
                        "file": r.file,
                        "depth": r.depth,
                        "pagerank": r.pagerank,
                    })
                })
                .collect();
            Ok(Value::Array(arr))
        })
    }
}

/// `synapse_rank`: top symbols by PageRank over the CALLS graph.
pub struct RankTool;

#[async_trait::async_trait]
impl McpTool for RankTool {
    fn name(&self) -> &'static str {
        "synapse_rank"
    }
    fn description(&self) -> &'static str {
        "List top symbols by PageRank over the CALLS graph."
    }
    async fn invoke(&self, args: Value) -> Result<Value, McpToolError> {
        let top = require_u64(&args, "top")
            .map(|n| usize::try_from(n).unwrap_or(usize::MAX))
            .unwrap_or(25);
        let kind = args
            .get("kind")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let db_path = resolve_db_path(args.get("repo"))?;
        with_conn(&db_path, |conn| {
            let (query, params): (&str, Vec<(&str, lbug::Value)>) = match kind.as_deref() {
                Some(k) => (
                    "MATCH (s:Symbol) WHERE s.pagerank IS NOT NULL AND s.kind = $kind \
                     RETURN s.id, s.name, s.kind, s.pagerank",
                    vec![("kind", lbug::Value::String(k.to_string()))],
                ),
                None => (
                    "MATCH (s:Symbol) WHERE s.pagerank IS NOT NULL \
                     RETURN s.id, s.name, s.kind, s.pagerank",
                    vec![],
                ),
            };
            let mut stmt = conn
                .prepare(query)
                .map_err(|e| McpToolError::Internal(format!("prepare: {e}")))?;
            let result = conn
                .execute(&mut stmt, params)
                .map_err(|e| McpToolError::Internal(format!("execute: {e}")))?;
            let mut scored: Vec<(String, String, String, f64)> = Vec::new();
            for row in result {
                if let (
                    Some(lbug::Value::String(id)),
                    Some(lbug::Value::String(name)),
                    Some(lbug::Value::String(k)),
                    Some(lbug::Value::Double(score)),
                ) = (row.first(), row.get(1), row.get(2), row.get(3))
                {
                    scored.push((id.clone(), name.clone(), k.clone(), *score));
                }
            }
            scored.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap_or(std::cmp::Ordering::Equal));
            scored.truncate(top);
            let arr: Vec<Value> = scored
                .into_iter()
                .map(|(id, name, k, score)| {
                    let file_path = id.split("::").next().unwrap_or(&id).to_string();
                    json!({
                        "symbol": name,
                        "kind": k,
                        "file": file_path,
                        "pagerank": score,
                    })
                })
                .collect();
            Ok(Value::Array(arr))
        })
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::expect_used)] // test fixtures assert on Option/Result values
mod tests {
    use super::*;
    use crate::mcp::repo_registry::RepoEntry;
    use std::path::Path;
    use std::sync::Mutex;

    /// Serializes tests that mutate the `HOME` env var.
    static HOME_LOCK: Mutex<()> = Mutex::new(());

    /// RAII guard that restores `HOME` on drop. Prevents test pollution if
    /// the test panics before manual restoration.
    struct HomeGuard {
        prev: Option<std::ffi::OsString>,
    }

    impl HomeGuard {
        fn set(value: &Path) -> Self {
            let prev = std::env::var_os("HOME");
            std::env::set_var("HOME", value);
            Self { prev }
        }

        /// Capture current HOME and `remove_var` it. On drop, restore HOME to
        /// its captured value (or leave unset if it was unset).
        fn unset() -> Self {
            let prev = std::env::var_os("HOME");
            std::env::remove_var("HOME");
            Self { prev }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match self.prev.take() {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
    }

    /// RAII guard that restores the current working directory on drop.
    struct CwdGuard {
        prev: std::path::PathBuf,
    }

    impl CwdGuard {
        fn set(path: &Path) -> std::io::Result<Self> {
            let prev = std::env::current_dir()?;
            std::env::set_current_dir(path)?;
            Ok(Self { prev })
        }
    }

    impl Drop for CwdGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.prev);
        }
    }

    #[test]
    fn cypher_tool_metadata() {
        let t = CypherTool;
        assert_eq!(t.name(), "synapse_cypher");
        assert!(t.description().contains("Cypher"));
    }

    #[test]
    fn callers_tool_metadata() {
        let t = CallersTool;
        assert_eq!(t.name(), "synapse_callers");
        assert!(t.description().contains("callers"));
    }

    #[test]
    fn callees_tool_metadata() {
        let t = CalleesTool;
        assert_eq!(t.name(), "synapse_callees");
        assert!(t.description().contains("callees"));
    }

    #[test]
    fn deps_tool_metadata() {
        let t = DepsTool;
        assert_eq!(t.name(), "synapse_deps");
        assert!(t.description().contains("imports"));
    }

    #[test]
    fn context_tool_metadata() {
        let t = ContextTool;
        assert_eq!(t.name(), "synapse_context");
        assert!(t.description().contains("intelligence"));
    }

    #[test]
    fn query_tool_metadata() {
        let t = QueryTool;
        assert_eq!(t.name(), "synapse_query");
        assert!(t.description().contains("Semantic"));
    }

    #[test]
    fn impact_tool_metadata() {
        let t = ImpactTool;
        assert_eq!(t.name(), "synapse_impact");
        assert!(t.description().contains("PageRank") || t.description().contains("transitively"));
    }

    #[test]
    fn rank_tool_metadata() {
        let t = RankTool;
        assert_eq!(t.name(), "synapse_rank");
        assert!(t.description().contains("PageRank"));
    }

    #[test]
    fn require_string_returns_error_for_missing_key() {
        let args = json!({});
        let err = require_string(&args, "name").unwrap_err();
        assert!(matches!(err, McpToolError::InvalidParams(_)));
    }

    #[test]
    fn require_string_returns_value_for_present_key() {
        let args = json!({"name": "parse_source"});
        assert_eq!(require_string(&args, "name").unwrap(), "parse_source");
    }

    #[test]
    fn require_u64_returns_none_for_missing_key() {
        let args = json!({});
        assert!(require_u64(&args, "top").is_none());
    }

    #[test]
    fn require_u64_returns_some_for_present_key() {
        let args = json!({"top": 25});
        assert_eq!(require_u64(&args, "top"), Some(25));
    }

    #[test]
    fn classify_query_error_ambiguous_maps_to_invalid_params() {
        let err: Box<dyn std::error::Error> = "matches multiple candidates".to_string().into();
        let mapped = classify_query_error(err);
        assert!(matches!(mapped, McpToolError::InvalidParams(_)));
    }

    #[test]
    fn classify_query_error_no_match_maps_to_not_found() {
        let err: Box<dyn std::error::Error> = "No symbol matches 'foo'".to_string().into();
        let mapped = classify_query_error(err);
        assert!(matches!(mapped, McpToolError::NotFound(_)));
    }

    #[test]
    fn classify_query_error_other_maps_to_internal() {
        let err: Box<dyn std::error::Error> = "unexpected db error".to_string().into();
        let mapped = classify_query_error(err);
        assert!(matches!(mapped, McpToolError::Internal(_)));
    }

    #[test]
    fn resolve_db_path_uses_registry_entry() {
        let _lock = HOME_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("synapse.lbug");
        std::fs::File::create(&db_path).unwrap();

        let mut registry = RepoRegistry::default();
        registry
            .add_or_update(
                RepoEntry {
                    name: "test".to_string(),
                    path: tmp.path().to_path_buf(),
                    db_path: db_path.clone(),
                    indexed_at: "2026-06-21T00:00:00Z".to_string(),
                    indexed_commit: String::new(),
                },
                false,
            )
            .unwrap();
        registry
            .save_to(&tmp.path().join(".synapse/repos.json"))
            .unwrap();

        let _home = HomeGuard::set(tmp.path());
        let result = resolve_db_path(Some(&json!("test"))).unwrap();

        assert_eq!(result, db_path);
    }

    /// Assert that a result is `Err(InvalidParams)` with `needle` in the message.
    fn assert_invalid_params_contains(result: Result<PathBuf, McpToolError>, needle: &str) {
        assert!(
            matches!(result, Err(McpToolError::InvalidParams(_))),
            "expected Err(InvalidParams), got {result:?}"
        );
        if let Err(McpToolError::InvalidParams(msg)) = result {
            assert!(
                msg.contains(needle),
                "InvalidParams message should contain '{needle}', got: {msg}"
            );
        }
    }

    /// Assert that a result is `Err(NotFound)` with `needle` in the message.
    fn assert_not_found_contains(result: Result<PathBuf, McpToolError>, needle: &str) {
        assert!(
            matches!(result, Err(McpToolError::NotFound(_))),
            "expected Err(NotFound), got {result:?}"
        );
        if let Err(McpToolError::NotFound(msg)) = result {
            assert!(
                msg.contains(needle),
                "NotFound message should contain '{needle}', got: {msg}"
            );
        }
    }

    #[test]
    fn resolve_db_path_rejects_unknown_name() {
        let _lock = HOME_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let _home = HomeGuard::set(tmp.path());

        assert_not_found_contains(
            resolve_db_path(Some(&json!("missing"))),
            "not found in registry",
        );
    }

    #[test]
    fn resolve_db_path_rejects_missing_db_file() {
        let _lock = HOME_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();

        let missing = tmp.path().join("nonexistent.lbug");
        let mut registry = RepoRegistry::default();
        registry
            .add_or_update(
                RepoEntry {
                    name: "test".to_string(),
                    path: tmp.path().to_path_buf(),
                    db_path: missing,
                    indexed_at: "2026-06-21T00:00:00Z".to_string(),
                    indexed_commit: String::new(),
                },
                false,
            )
            .unwrap();
        registry
            .save_to(&tmp.path().join(".synapse/repos.json"))
            .unwrap();

        let _home = HomeGuard::set(tmp.path());

        assert_invalid_params_contains(resolve_db_path(Some(&json!("test"))), "not found for repo");
    }

    #[test]
    fn resolve_db_path_empty_string_treated_as_none() {
        let _lock = HOME_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        std::fs::File::create(tmp.path().join("synapse.lbug")).unwrap();

        let _home = HomeGuard::set(tmp.path());
        let _cwd = CwdGuard::set(tmp.path()).unwrap();

        let result = resolve_db_path(Some(&json!(""))).unwrap();
        assert_eq!(result, tmp.path().join("synapse.lbug"));
    }

    #[test]
    fn resolve_db_path_no_repo_no_cwd_db_errors() {
        let _lock = HOME_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();

        let _home = HomeGuard::set(tmp.path());
        let _cwd = CwdGuard::set(tmp.path()).unwrap();

        assert_invalid_params_contains(resolve_db_path(None), "see synapse://repos");
    }

    #[test]
    fn resolve_db_path_registry_load_error_is_internal() {
        let _lock = HOME_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".synapse")).unwrap();
        std::fs::write(
            tmp.path().join(".synapse/repos.json"),
            "this is not valid JSON {{{",
        )
        .unwrap();

        let _home = HomeGuard::set(tmp.path());

        let result = resolve_db_path(Some(&json!("anything")));
        assert!(
            matches!(result, Err(McpToolError::Internal(_))),
            "expected Err(Internal), got {result:?}"
        );
    }

    #[test]
    fn resolve_db_path_no_cross_contamination_with_two_repos() {
        let _lock = HOME_LOCK.lock().unwrap();
        let tmp_home = tempfile::tempdir().unwrap();
        let tmp_a = tempfile::tempdir().unwrap();
        let tmp_b = tempfile::tempdir().unwrap();

        let db_a = tmp_a.path().join("synapse.lbug");
        let db_b = tmp_b.path().join("synapse.lbug");
        std::fs::File::create(&db_a).unwrap();
        std::fs::File::create(&db_b).unwrap();

        let mut registry = RepoRegistry::default();
        registry
            .add_or_update(
                RepoEntry {
                    name: "alpha".to_string(),
                    path: tmp_a.path().to_path_buf(),
                    db_path: db_a.clone(),
                    indexed_at: "2026-06-21T00:00:00Z".to_string(),
                    indexed_commit: String::new(),
                },
                false,
            )
            .unwrap();
        registry
            .add_or_update(
                RepoEntry {
                    name: "beta".to_string(),
                    path: tmp_b.path().to_path_buf(),
                    db_path: db_b.clone(),
                    indexed_at: "2026-06-21T00:00:00Z".to_string(),
                    indexed_commit: String::new(),
                },
                false,
            )
            .unwrap();
        registry
            .save_to(&tmp_home.path().join(".synapse/repos.json"))
            .unwrap();

        let _home = HomeGuard::set(tmp_home.path());

        let result_a = resolve_db_path(Some(&json!("alpha"))).unwrap();
        let result_b = resolve_db_path(Some(&json!("beta"))).unwrap();

        assert_eq!(result_a, db_a);
        assert_eq!(result_b, db_b);
        assert_ne!(db_a, db_b);
    }

    #[test]
    fn concurrent_resolve_db_path_doesnt_deadlock() {
        // HOME_LOCK serializes tests that mutate HOME (process-global env var).
        // Without it, a parallel test could clobber HOME between this test setting
        // it and the spawned threads reading it.
        let _lock = HOME_LOCK.lock().unwrap();
        let tmp_home = tempfile::tempdir().unwrap();
        let tmp_repo = tempfile::tempdir().unwrap();
        let db_path = tmp_repo.path().join("synapse.lbug");
        std::fs::File::create(&db_path).unwrap();

        let mut registry = RepoRegistry::default();
        registry
            .add_or_update(
                RepoEntry {
                    name: "test".to_string(),
                    path: tmp_repo.path().to_path_buf(),
                    db_path: db_path.clone(),
                    indexed_at: "2026-06-21T00:00:00Z".to_string(),
                    indexed_commit: String::new(),
                },
                false,
            )
            .unwrap();
        registry
            .save_to(&tmp_home.path().join(".synapse/repos.json"))
            .unwrap();

        let _home = HomeGuard::set(tmp_home.path());
        let name = json!("test");

        let results: Vec<PathBuf> = std::thread::scope(|s| {
            (0..8)
                .map(|_| s.spawn(|| resolve_db_path(Some(&name)).unwrap()))
                .collect::<Vec<_>>()
                .into_iter()
                .map(|h| h.join().unwrap())
                .collect()
        });

        for result in &results {
            assert_eq!(*result, db_path);
        }
    }

    #[test]
    fn resolve_db_path_home_unset_returns_internal() {
        let _lock = HOME_LOCK.lock().unwrap();

        let _home = HomeGuard::unset();

        // HOME is unset. resolve_db_path with a non-empty `repo` arg forces
        // RepoRegistry::load() → RegistryError::NoHome → McpToolError::Internal
        // with the documented "HOME is unset" message.
        let result = resolve_db_path(Some(&json!("anything")));

        let matches_expected = matches!(
            &result,
            Err(McpToolError::Internal(msg)) if msg.contains("HOME is unset"),
        );
        assert!(
            matches_expected,
            "expected Internal mentioning HOME unset, got {result:?}"
        );
    }
}
