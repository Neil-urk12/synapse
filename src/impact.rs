use crate::query::candidates::resolve_symbol_candidates;
use lbug::{Connection, Value};
use std::collections::{HashMap, HashSet, VecDeque};

/// Errors that can arise from `run_impact` and its helpers.
#[derive(Debug)]
pub enum ImpactError {
    /// The target name didn't match any symbol (fuzzy). Carries the
    /// `Did you mean?` candidates so the caller can render a useful error.
    NoMatch {
        target: String,
        suggestions: Vec<String>,
    },
    /// DB / I/O failure.
    Io(lbug::Error),
}

impl std::fmt::Display for ImpactError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImpactError::NoMatch {
                target,
                suggestions,
            } => {
                write!(f, "Symbol '{}' not found", target)?;
                if !suggestions.is_empty() {
                    write!(f, ". Did you mean:")?;
                    for s in suggestions {
                        write!(f, "\n  - {}", s)?;
                    }
                }
                Ok(())
            }
            ImpactError::Io(e) => write!(f, "{}", e),
        }
    }
}

impl std::error::Error for ImpactError {}

impl From<lbug::Error> for ImpactError {
    fn from(e: lbug::Error) -> Self {
        ImpactError::Io(e)
    }
}

/// Options controlling BFS traversal and output truncation.
#[derive(Debug, Clone)]
pub struct ImpactOptions {
    /// Maximum BFS depth (1 = direct callers only). `None` = unlimited.
    pub max_depth: Option<usize>,
    /// Maximum number of impacted symbols returned (after PageRank sort). `None` = unlimited.
    pub top: Option<usize>,
}

impl Default for ImpactOptions {
    fn default() -> Self {
        // Defaults match the CLI surface in `src/main.rs::Impact`.
        Self {
            max_depth: None,
            top: Some(50),
        }
    }
}

/// One row of the impact result: a symbol reachable from the target via
/// reverse CALLS reachability, annotated with depth and PageRank.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ImpactRow {
    /// 1-based rank in the final sorted output (set after sort/truncate).
    pub rank: usize,
    /// Symbol id (`file_path::name` or `parent_id::name`).
    pub id: String,
    /// Bare symbol name (last segment of `id`).
    pub name: String,
    /// Symbol kind (Function, Method, Struct, etc.).
    pub kind: String,
    /// First segment of `id` — the file the symbol lives in.
    pub file: String,
    /// Start line in the source file.
    pub start_line: usize,
    /// Shortest path length from the target symbol. Direct callers = 1.
    pub depth: usize,
    /// PageRank score at the time of query. `None` if the column is unset
    /// (pre-PageRank DB) — those rows sort to the bottom of the result.
    pub pagerank: Option<f64>,
}

/// One row of symbol metadata loaded from the DB. `impact` needs a superset
/// of `SymbolInfo` (it also carries `pagerank`), so it has its own type.
#[derive(Debug, Clone)]
/// `pub` because it appears in the signature of `compute_impact` (a `pub` function).
pub struct SymbolRow {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub start_line: usize,
    pub pagerank: Option<f64>,
}

/// Compute the transitive reverse-reachability of `target` over the CALLS graph.
///
/// `incoming[caller_id]` is the set of symbol ids that call `caller_id`
/// (i.e. reverse-CALLS adjacency). `rows` is the metadata for every symbol
/// the algorithm might surface. Returns impacted rows with depth and
/// PageRank.
///
/// `compute_impact` honours `opts.max_depth` so the BFS doesn't visit
/// unreachable nodes. It does NOT sort by PageRank or apply `opts.top` —
/// those happen in `sort_and_rank` / `run_impact`.
///
/// `target` is excluded from the result (a symbol is not its own impact).
pub fn compute_impact(
    target: &str,
    incoming: &HashMap<String, HashSet<String>>,
    rows: &HashMap<String, SymbolRow>,
    opts: &ImpactOptions,
) -> Vec<ImpactRow> {
    let mut visited: HashSet<String> = HashSet::new();
    visited.insert(target.to_string());
    let mut queue: VecDeque<(String, usize)> = VecDeque::new();
    queue.push_back((target.to_string(), 0));
    let mut out: Vec<ImpactRow> = Vec::new();

    while let Some((node, depth)) = queue.pop_front() {
        // Stop expanding past the depth cap (but still record the row at the cap).
        let next_depth = depth + 1;
        if let Some(cap) = opts.max_depth {
            if depth >= cap {
                continue;
            }
        }
        let Some(callers) = incoming.get(&node) else {
            continue;
        };
        for caller in callers {
            if !visited.insert(caller.clone()) {
                continue;
            }
            let row = match rows.get(caller) {
                Some(r) => ImpactRow {
                    rank: 0, // assigned later, after sort
                    id: r.id.clone(),
                    name: r.name.clone(),
                    kind: r.kind.clone(),
                    file: r.id.split("::").next().unwrap_or(&r.id).to_string(),
                    start_line: r.start_line,
                    depth: next_depth,
                    pagerank: r.pagerank,
                },
                None => continue, // dangling CALLS target — defensive
            };
            queue.push_back((caller.clone(), next_depth));
            out.push(row);
        }
    }
    out
}

/// Sort `rows` in place by PageRank DESC. Symbols with `pagerank = None`
/// sort to the bottom. Ties on PageRank break by `id` ASC for deterministic
/// output across runs. Truncates to `opts.top` (if `Some`). After this call,
/// every row's `rank` field is set to its 1-based position in the sorted list.
pub fn sort_and_rank(rows: &mut Vec<ImpactRow>, opts: &ImpactOptions) {
    rows.sort_by(|a, b| match (a.pagerank, b.pagerank) {
        (Some(x), Some(y)) => y
            .partial_cmp(&x)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.id.cmp(&b.id)),
        (Some(_), None) => std::cmp::Ordering::Less, // Non-null wins
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.id.cmp(&b.id),
    });
    if let Some(top) = opts.top {
        rows.truncate(top);
    }
    for (i, row) in rows.iter_mut().enumerate() {
        row.rank = i + 1;
    }
}

/// Load every symbol's metadata + PageRank from the DB.
pub fn load_symbol_rows(conn: &Connection) -> Result<HashMap<String, SymbolRow>, ImpactError> {
    let mut stmt =
        conn.prepare("MATCH (s:Symbol) RETURN s.id, s.name, s.kind, s.start_line, s.pagerank")?;
    let result = conn.execute(&mut stmt, vec![])?;
    let mut rows = HashMap::new();
    for row in result {
        // LadybugDB may return `Value::Null` for `s.pagerank` on pre-PageRank DBs.
        let id = match row.first() {
            Some(Value::String(s)) => s.clone(),
            _ => continue,
        };
        let name = match row.get(1) {
            Some(Value::String(s)) => s.clone(),
            _ => continue,
        };
        let kind = match row.get(2) {
            Some(Value::String(s)) => s.clone(),
            _ => continue,
        };
        let start_line = match row.get(3) {
            Some(Value::Int64(n)) => usize::try_from(*n).unwrap_or(0),
            _ => 0,
        };
        let pagerank = match row.get(4) {
            Some(Value::Double(d)) => Some(*d),
            _ => None, // Null or any other type → None
        };
        rows.insert(
            id.clone(),
            SymbolRow {
                id,
                name,
                kind,
                start_line,
                pagerank,
            },
        );
    }
    Ok(rows)
}

/// Load every CALLS edge from the DB and return the inverted adjacency map
/// (`incoming[callee] = {callers...}`).
pub fn load_incoming_calls(
    conn: &Connection,
) -> Result<HashMap<String, HashSet<String>>, ImpactError> {
    let mut stmt = conn.prepare("MATCH (a:Symbol)-[:CALLS]->(b:Symbol) RETURN a.id, b.id")?;
    let result = conn.execute(&mut stmt, vec![])?;
    let mut incoming: HashMap<String, HashSet<String>> = HashMap::new();
    for row in result {
        if let (Some(Value::String(from)), Some(Value::String(to))) = (row.first(), row.get(1)) {
            incoming.entry(to.clone()).or_default().insert(from.clone());
        }
    }
    Ok(incoming)
}

/// Orchestrator: load everything from the DB, resolve `target` via fuzzy
/// candidate match, run `compute_impact`, sort and rank, return.
pub fn run_impact(
    conn: &Connection,
    target: &str,
    fuzzy: bool,
    opts: &ImpactOptions,
) -> Result<Vec<ImpactRow>, ImpactError> {
    let rows = load_symbol_rows(conn)?;
    if rows.is_empty() {
        return Err(ImpactError::NoMatch {
            target: target.to_string(),
            suggestions: vec![],
        });
    }

    // Build a pool of `SymbolInfo`-shaped records for the existing candidate
    // resolver. We only need `id` and `name` for fuzzy match, so we project.
    let pool: Vec<crate::types::query::SymbolInfo> = rows
        .values()
        .map(|r| crate::types::query::SymbolInfo {
            id: r.id.clone(),
            name: r.name.clone(),
            kind: r.kind.clone(),
            start_line: r.start_line,
            end_line: r.start_line,   // unused by resolver
            signature: String::new(), // unused by resolver
        })
        .collect();
    let candidates = resolve_symbol_candidates(target, fuzzy, &pool);
    if candidates.is_empty() {
        // Surface up to 5 closest by simple "name contains target substring"
        // (case-insensitive) as suggestions.
        let lc = target.to_lowercase();
        let mut suggestions: Vec<String> = rows
            .values()
            .filter(|r| r.name.to_lowercase().contains(&lc) || r.id.to_lowercase().contains(&lc))
            .map(|r| r.id.clone())
            .collect();
        suggestions.sort();
        suggestions.truncate(5);
        return Err(ImpactError::NoMatch {
            target: target.to_string(),
            suggestions,
        });
    }
    let resolved_id = candidates[0].id.clone();
    if candidates.len() > 1 {
        eprintln!(
            "Warning: {} matches for '{}'; using the first: {}",
            candidates.len(),
            target,
            resolved_id
        );
    }

    let incoming = load_incoming_calls(conn)?;
    let mut result = compute_impact(&resolved_id, &incoming, &rows, opts);
    sort_and_rank(&mut result, opts);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, pagerank: Option<f64>) -> SymbolRow {
        let name = id.rsplit("::").next().unwrap_or(id).to_string();
        SymbolRow {
            id: id.to_string(),
            name,
            kind: "Function".to_string(),
            start_line: 1,
            pagerank,
        }
    }

    fn rows_of(ids: &[&str]) -> HashMap<String, SymbolRow> {
        ids.iter().map(|i| (i.to_string(), row(i, None))).collect()
    }

    fn incoming_of(pairs: &[(&str, &str)]) -> HashMap<String, HashSet<String>> {
        // pairs are (caller, callee) — i.e. forward CALLS. We invert here.
        let mut m: HashMap<String, HashSet<String>> = HashMap::new();
        for (caller, callee) in pairs {
            m.entry(callee.to_string())
                .or_default()
                .insert(caller.to_string());
        }
        m
    }

    #[test]
    fn test_impact_direct_callers() {
        // A → B
        let incoming = incoming_of(&[("A", "B")]);
        let rows = rows_of(&["A", "B"]);
        let opts = ImpactOptions {
            max_depth: None,
            top: None,
        };
        let out = compute_impact("B", &incoming, &rows, &opts);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "A");
        assert_eq!(out[0].depth, 1);
    }

    #[test]
    fn test_impact_transitive_chain() {
        // A → B → C → D
        let incoming = incoming_of(&[("A", "B"), ("B", "C"), ("C", "D")]);
        let rows = rows_of(&["A", "B", "C", "D"]);
        let opts = ImpactOptions {
            max_depth: None,
            top: None,
        };
        let out = compute_impact("D", &incoming, &rows, &opts);
        assert_eq!(out.len(), 3);
        let by_id: HashMap<String, usize> = out.iter().map(|r| (r.id.clone(), r.depth)).collect();
        assert_eq!(by_id["C"], 1);
        assert_eq!(by_id["B"], 2);
        assert_eq!(by_id["A"], 3);
    }

    #[test]
    fn test_impact_cycle() {
        // A → B → A, target = A
        let incoming = incoming_of(&[("A", "B"), ("B", "A")]);
        let rows = rows_of(&["A", "B"]);
        let opts = ImpactOptions {
            max_depth: None,
            top: None,
        };
        let out = compute_impact("A", &incoming, &rows, &opts);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "B");
        assert_eq!(out[0].depth, 1);
    }

    #[test]
    fn test_impact_diamond() {
        // A → B → D, A → C → D
        let incoming = incoming_of(&[("A", "B"), ("A", "C"), ("B", "D"), ("C", "D")]);
        let rows = rows_of(&["A", "B", "C", "D"]);
        let opts = ImpactOptions {
            max_depth: None,
            top: None,
        };
        let out = compute_impact("D", &incoming, &rows, &opts);
        assert_eq!(out.len(), 3, "A, B, C each appear once");
        let by_id: HashMap<String, usize> = out.iter().map(|r| (r.id.clone(), r.depth)).collect();
        assert_eq!(by_id["B"], 1);
        assert_eq!(by_id["C"], 1);
        assert_eq!(by_id["A"], 2);
    }

    #[test]
    fn test_impact_no_callers() {
        // Orphan target
        let incoming: HashMap<String, HashSet<String>> = HashMap::new();
        let rows = rows_of(&["A", "B"]);
        let opts = ImpactOptions {
            max_depth: None,
            top: None,
        };
        let out = compute_impact("A", &incoming, &rows, &opts);
        assert!(out.is_empty());
    }

    #[test]
    fn test_impact_self_recursion() {
        // A → A, target = A — self is excluded
        let incoming = incoming_of(&[("A", "A")]);
        let rows = rows_of(&["A"]);
        let opts = ImpactOptions {
            max_depth: None,
            top: None,
        };
        let out = compute_impact("A", &incoming, &rows, &opts);
        assert!(out.is_empty());
    }

    #[test]
    fn test_impact_max_depth_caps() {
        // A → B → C → D → E, target = E
        let incoming = incoming_of(&[("A", "B"), ("B", "C"), ("C", "D"), ("D", "E")]);
        let rows = rows_of(&["A", "B", "C", "D", "E"]);
        let opts = ImpactOptions {
            max_depth: Some(2),
            top: None,
        };
        let out = compute_impact("E", &incoming, &rows, &opts);
        let by_id: HashMap<String, usize> = out.iter().map(|r| (r.id.clone(), r.depth)).collect();
        assert_eq!(by_id["D"], 1);
        assert_eq!(by_id["C"], 2);
        assert!(!by_id.contains_key("B"), "B is at depth 3, beyond the cap");
        assert!(!by_id.contains_key("A"));
    }

    #[test]
    fn test_impact_max_depth_zero() {
        // No edges traversed, empty result.
        let incoming = incoming_of(&[("A", "B")]);
        let rows = rows_of(&["A", "B"]);
        let opts = ImpactOptions {
            max_depth: Some(0),
            top: None,
        };
        let out = compute_impact("B", &incoming, &rows, &opts);
        assert!(out.is_empty());
    }

    fn make_row(id: &str, depth: usize, pagerank: Option<f64>) -> ImpactRow {
        ImpactRow {
            rank: 0,
            id: id.to_string(),
            name: id.to_string(),
            kind: "Function".to_string(),
            file: "f".to_string(),
            start_line: 1,
            depth,
            pagerank,
        }
    }

    #[test]
    fn test_sort_pagerank_desc() {
        let mut rows = vec![
            make_row("a", 1, Some(0.01)),
            make_row("b", 1, Some(0.10)),
            make_row("c", 1, Some(0.05)),
        ];
        let opts = ImpactOptions::default();
        sort_and_rank(&mut rows, &opts);
        assert_eq!(rows[0].id, "b");
        assert_eq!(rows[1].id, "c");
        assert_eq!(rows[2].id, "a");
        assert_eq!(rows[0].rank, 1);
        assert_eq!(rows[1].rank, 2);
        assert_eq!(rows[2].rank, 3);
    }

    #[test]
    fn test_sort_null_pagerank_last_with_id_tiebreak() {
        let mut rows = vec![
            make_row("z", 1, None),
            make_row("a", 1, None),
            make_row("m", 1, Some(0.5)),
        ];
        let opts = ImpactOptions::default();
        sort_and_rank(&mut rows, &opts);
        assert_eq!(rows[0].id, "m", "non-null PageRank ranks first");
        assert_eq!(rows[1].id, "a", "nulls tiebreak by id ASC");
        assert_eq!(rows[2].id, "z");
    }

    #[test]
    fn test_sort_truncates_to_top() {
        let mut rows: Vec<ImpactRow> = (0..100)
            .map(|i| make_row(&format!("s{i}"), 1, Some(1.0 / (f64::from(i) + 1.0))))
            .collect();
        let opts = ImpactOptions {
            max_depth: None,
            top: Some(10),
        };
        sort_and_rank(&mut rows, &opts);
        assert_eq!(rows.len(), 10);
        assert_eq!(rows[0].rank, 1);
        assert_eq!(rows[9].rank, 10);
    }

    #[test]
    fn test_impact_dangling_callers_skipped() {
        // A → B; rows only has B (A was deleted)
        let incoming = incoming_of(&[("A", "B")]);
        let rows = rows_of(&["B"]);
        let opts = ImpactOptions {
            max_depth: None,
            top: None,
        };
        let out = compute_impact("B", &incoming, &rows, &opts);
        assert!(out.is_empty());
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use lbug::{Connection, Database, SystemConfig};
    use tempfile::tempdir;

    /// Populate `conn` with a small graph: a → b → c (forward CALLS),
    /// with d → a (so a is impacted by d). PageRank scores are written
    /// directly so the sort can be verified.
    fn populate_test_graph(conn: &Connection) {
        conn.query(
            "CREATE (:Symbol {id: 'a', name: 'a', kind: 'Function', start_line: 1, end_line: 5, signature: 'fn a()', raw_calls: '[]', pagerank: 0.50})",
        )
        .unwrap();
        conn.query(
            "CREATE (:Symbol {id: 'b', name: 'b', kind: 'Function', start_line: 1, end_line: 5, signature: 'fn b()', raw_calls: '[]', pagerank: 0.30})",
        )
        .unwrap();
        conn.query(
            "CREATE (:Symbol {id: 'c', name: 'c', kind: 'Function', start_line: 1, end_line: 5, signature: 'fn c()', raw_calls: '[]', pagerank: 0.20})",
        )
        .unwrap();
        conn.query(
            "CREATE (:Symbol {id: 'd', name: 'd', kind: 'Function', start_line: 1, end_line: 5, signature: 'fn d()', raw_calls: '[]', pagerank: 0.10})",
        )
        .unwrap();
        conn.query(
            "MATCH (x:Symbol {id: 'a'}), (y:Symbol {id: 'b'}) CREATE (x)-[:CALLS {call_site_line: 1}]->(y)",
        )
        .unwrap();
        conn.query(
            "MATCH (x:Symbol {id: 'b'}), (y:Symbol {id: 'c'}) CREATE (x)-[:CALLS {call_site_line: 1}]->(y)",
        )
        .unwrap();
        conn.query(
            "MATCH (x:Symbol {id: 'd'}), (y:Symbol {id: 'a'}) CREATE (x)-[:CALLS {call_site_line: 1}]->(y)",
        )
        .unwrap();
    }

    /// Run `test` with a fresh, populated test connection. The tempdir,
    /// database, and connection all live inside the closure so they're
    /// dropped at the end of each test (no leaks across parallel runs).
    fn with_conn<F: FnOnce(&Connection)>(test: F) {
        let tmp = tempdir().unwrap();
        let db_path = tmp.path().join("impact_e2e.lbug");
        let db = Database::new(&db_path, SystemConfig::default()).unwrap();
        let conn = Connection::new(&db).unwrap();
        crate::schema::init_schema(&conn).unwrap();
        populate_test_graph(&conn);
        test(&conn);
    }

    #[test]
    fn test_run_impact_transitive_sorted_by_pagerank() {
        with_conn(|conn| {
            let opts = ImpactOptions::default();
            let rows = run_impact(conn, "c", true, &opts).unwrap();
            // Impact of c: b (depth 1), a (depth 2), d (depth 3)
            assert_eq!(rows.len(), 3);
            assert_eq!(rows[0].id, "a");
            assert_eq!(rows[0].rank, 1);
            assert_eq!(rows[0].depth, 2);
            assert_eq!(rows[1].id, "b");
            assert_eq!(rows[1].rank, 2);
            assert_eq!(rows[1].depth, 1);
            assert_eq!(rows[2].id, "d");
            assert_eq!(rows[2].rank, 3);
            assert_eq!(rows[2].depth, 3);
        });
    }

    #[test]
    fn test_run_impact_unknown_target_returns_no_match() {
        with_conn(|conn| {
            let opts = ImpactOptions::default();
            let res = run_impact(conn, "nonexistent_symbol_xyz", true, &opts);
            assert!(res.is_err());
            let err = res.unwrap_err();
            let msg = err.to_string();
            assert!(msg.contains("not found"), "actual: {}", msg);
        });
    }

    #[test]
    fn test_run_impact_max_depth_filters() {
        with_conn(|conn| {
            let opts = ImpactOptions {
                max_depth: Some(1),
                top: None,
            };
            let rows = run_impact(conn, "c", true, &opts).unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].id, "b");
        });
    }

    #[test]
    fn test_run_impact_top_truncates() {
        with_conn(|conn| {
            let opts = ImpactOptions {
                max_depth: None,
                top: Some(2),
            };
            let rows = run_impact(conn, "c", true, &opts).unwrap();
            assert_eq!(rows.len(), 2);
            assert_eq!(rows[0].rank, 1);
            assert_eq!(rows[1].rank, 2);
        });
    }
}
