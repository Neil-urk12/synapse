use std::collections::{HashMap, HashSet};

use crate::impact::SymbolRow;

/// Options for dead code detection.
#[derive(Debug, Clone)]
pub struct DeadCodeOptions {
    /// Maximum number of results to return. `None` = no limit.
    pub top: Option<usize>,
    /// Filter to a specific kind ("Function" or "Method"). `None` = both.
    pub kind: Option<String>,
}

impl Default for DeadCodeOptions {
    fn default() -> Self {
        Self {
            top: Some(100),
            kind: None,
        }
    }
}

/// One row of dead code detected by `find_dead_code`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DeadCodeRow {
    /// Symbol id (`file_path::name` or `parent_id::name`).
    pub id: String,
    /// Bare symbol name (last segment of `id`).
    pub name: String,
    /// Symbol kind (Function, Method).
    pub kind: String,
    /// First segment of `id` — the file the symbol lives in.
    pub file: String,
    /// Start line in the source file.
    pub start_line: usize,
    /// PageRank score at the time of query.
    pub pagerank: Option<f64>,
}

/// Exact-match entry point names that should never be reported as dead.
const ENTRY_POINT_PATTERNS: &[&str] = &[
    "main", "init", "__init__", "setup", "teardown", "setUp", "tearDown",
];

/// Returns `true` if `name` matches an entry point pattern and should be
/// excluded from dead code results.
pub fn is_entry_point(name: &str) -> bool {
    if ENTRY_POINT_PATTERNS.contains(&name) {
        return true;
    }
    // Prefix/suffix patterns
    name.starts_with("test_")
        || name.ends_with("_test")
        || name.starts_with("Test")
        || name == "test"
}

/// Find dead code — functions/methods with zero incoming CALLS edges.
///
/// `symbols` is the full set of Symbol rows from the DB.
/// `incoming` is the inverted CALLS adjacency map (`incoming[callee] = {callers...}`).
/// Symbols with an empty entry (or missing entry) in `incoming` have zero callers.
///
/// Results are filtered to Function/Method kinds, excluding entry points,
/// sorted by PageRank ascending (least important first), and truncated to
/// `opts.top`.
pub fn find_dead_code(
    symbols: &HashMap<String, SymbolRow>,
    incoming: &HashMap<String, HashSet<String>>,
    opts: &DeadCodeOptions,
) -> Vec<DeadCodeRow> {
    let mut dead: Vec<DeadCodeRow> = Vec::new();

    for (id, sym) in symbols {
        // Only consider Function and Method kinds.
        if sym.kind != "Function" && sym.kind != "Method" {
            continue;
        }

        // Exclude entry points by name.
        if is_entry_point(&sym.name) {
            continue;
        }

        // A symbol is dead if it has zero incoming CALLS edges.
        let has_callers = incoming.get(id).is_some_and(|callers| !callers.is_empty());
        if has_callers {
            continue;
        }

        // Apply kind filter if specified.
        if let Some(ref filter_kind) = opts.kind {
            if &sym.kind != filter_kind {
                continue;
            }
        }

        let file = id.split("::").next().unwrap_or(id).to_string();

        dead.push(DeadCodeRow {
            id: id.clone(),
            name: sym.name.clone(),
            kind: sym.kind.clone(),
            file,
            start_line: sym.start_line,
            pagerank: sym.pagerank,
        });
    }

    // Sort by pagerank ascending (least important first). Symbols without
    // pagerank sort to the top (treated as 0.0).
    dead.sort_by(|a, b| {
        let pa = a.pagerank.unwrap_or(0.0);
        let pb = b.pagerank.unwrap_or(0.0);
        pa.partial_cmp(&pb)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.id.cmp(&b.id))
    });

    // Truncate to top-N.
    if let Some(top) = opts.top {
        dead.truncate(top);
    }

    dead
}

/// Returns the list of entry point patterns for display in output.
pub fn entry_point_patterns() -> Vec<&'static str> {
    let mut patterns: Vec<&str> = ENTRY_POINT_PATTERNS.to_vec();
    patterns.extend_from_slice(&["test_*", "*_test", "Test*", "test"]);
    patterns
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: build symbol rows from (name, kind) pairs.
    /// Ids are generated as "file.rs::{name}".
    fn rows_of(entries: &[(&str, &str)]) -> HashMap<String, SymbolRow> {
        let mut map = HashMap::new();
        for (i, (name, kind)) in entries.iter().enumerate() {
            let id = format!("file.rs::{}", name);
            map.insert(
                id.clone(),
                SymbolRow {
                    id,
                    name: name.to_string(),
                    kind: kind.to_string(),
                    start_line: (i + 1) * 10,
                    pagerank: Some(0.001 * (i as f64 + 1.0)),
                },
            );
        }
        map
    }

    /// Helper: build incoming edges from (caller, callee) pairs.
    fn incoming_of(edges: &[(&str, &str)]) -> HashMap<String, HashSet<String>> {
        let mut map: HashMap<String, HashSet<String>> = HashMap::new();
        for (caller, callee) in edges {
            let caller_id = format!("file.rs::{}", caller);
            let callee_id = format!("file.rs::{}", callee);
            map.entry(callee_id).or_default().insert(caller_id);
        }
        map
    }

    #[test]
    fn test_basic_dead_code() {
        // F → E → D → A → B, C has zero callers → C is dead
        let incoming = incoming_of(&[("F", "E"), ("E", "D"), ("D", "A"), ("A", "B")]);
        let rows = rows_of(&[
            ("A", "Function"),
            ("B", "Function"),
            ("C", "Function"),
            ("D", "Function"),
            ("E", "Function"),
            ("F", "Function"),
        ]);
        let out = find_dead_code(&rows, &incoming, &DeadCodeOptions::default());
        assert_eq!(out.len(), 2); // C and F are dead
        assert_eq!(out[0].name, "C");
        assert_eq!(out[1].name, "F");
    }

    #[test]
    fn test_entry_point_main_excluded() {
        let incoming = incoming_of(&[]);
        let rows = rows_of(&[("main", "Function")]);
        let out = find_dead_code(&rows, &incoming, &DeadCodeOptions::default());
        assert_eq!(out.len(), 0);
    }

    #[test]
    fn test_entry_point_init_excluded() {
        let incoming = incoming_of(&[]);
        let rows = rows_of(&[("init", "Function")]);
        let out = find_dead_code(&rows, &incoming, &DeadCodeOptions::default());
        assert_eq!(out.len(), 0);
    }

    #[test]
    fn test_entry_point_test_prefix_excluded() {
        let incoming = incoming_of(&[]);
        let rows = rows_of(&[("test_parse", "Function")]);
        let out = find_dead_code(&rows, &incoming, &DeadCodeOptions::default());
        assert_eq!(out.len(), 0);
    }

    #[test]
    fn test_entry_point_test_suffix_excluded() {
        let incoming = incoming_of(&[]);
        let rows = rows_of(&[("parser_test", "Function")]);
        let out = find_dead_code(&rows, &incoming, &DeadCodeOptions::default());
        assert_eq!(out.len(), 0);
    }

    #[test]
    fn test_entry_point_test_camel_excluded() {
        let incoming = incoming_of(&[]);
        let rows = rows_of(&[("TestParse", "Function")]);
        let out = find_dead_code(&rows, &incoming, &DeadCodeOptions::default());
        assert_eq!(out.len(), 0);
    }

    #[test]
    fn test_entry_point_test_exact_excluded() {
        // JS/TS: test() function
        let incoming = incoming_of(&[]);
        let rows = rows_of(&[("test", "Function")]);
        let out = find_dead_code(&rows, &incoming, &DeadCodeOptions::default());
        assert_eq!(out.len(), 0);
    }

    #[test]
    fn test_entry_point_setup_excluded() {
        let incoming = incoming_of(&[]);
        let rows = rows_of(&[("setup", "Function")]);
        let out = find_dead_code(&rows, &incoming, &DeadCodeOptions::default());
        assert_eq!(out.len(), 0);
    }

    #[test]
    fn test_kind_filter_function_only() {
        let incoming = incoming_of(&[]);
        let rows = rows_of(&[("foo", "Function"), ("bar", "Method")]);
        let opts = DeadCodeOptions {
            kind: Some("Function".into()),
            ..Default::default()
        };
        let out = find_dead_code(&rows, &incoming, &opts);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "foo");
    }

    #[test]
    fn test_kind_filter_method_only() {
        let incoming = incoming_of(&[]);
        let rows = rows_of(&[("foo", "Function"), ("bar", "Method")]);
        let opts = DeadCodeOptions {
            kind: Some("Method".into()),
            ..Default::default()
        };
        let out = find_dead_code(&rows, &incoming, &opts);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "bar");
    }

    #[test]
    fn test_top_n_limits_results() {
        let incoming = incoming_of(&[]);
        let rows = rows_of(&[("a", "Function"), ("b", "Function"), ("c", "Function")]);
        let opts = DeadCodeOptions {
            top: Some(2),
            ..Default::default()
        };
        let out = find_dead_code(&rows, &incoming, &opts);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn test_self_call_not_dead() {
        // A calls itself → not dead
        let incoming = incoming_of(&[("A", "A")]);
        let rows = rows_of(&[("A", "Function")]);
        let out = find_dead_code(&rows, &incoming, &DeadCodeOptions::default());
        assert_eq!(out.len(), 0);
    }

    #[test]
    fn test_non_function_kinds_excluded() {
        // Struct with zero callers is not dead (not a Function/Method)
        let incoming = incoming_of(&[]);
        let rows = rows_of(&[("MyStruct", "Struct"), ("MyClass", "Class")]);
        let out = find_dead_code(&rows, &incoming, &DeadCodeOptions::default());
        assert_eq!(out.len(), 0);
    }

    #[test]
    fn test_sorted_by_pagerank_ascending() {
        let incoming = incoming_of(&[]);
        let mut rows = rows_of(&[("a", "Function"), ("b", "Function"), ("c", "Function")]);
        // Override pagerank to test sorting
        rows.get_mut("file.rs::a").unwrap().pagerank = Some(0.3);
        rows.get_mut("file.rs::b").unwrap().pagerank = Some(0.1);
        rows.get_mut("file.rs::c").unwrap().pagerank = Some(0.2);
        let out = find_dead_code(&rows, &incoming, &DeadCodeOptions::default());
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].name, "b"); // lowest pagerank first
        assert_eq!(out[1].name, "c");
        assert_eq!(out[2].name, "a");
    }

    #[test]
    fn test_is_entry_point() {
        assert!(is_entry_point("main"));
        assert!(is_entry_point("init"));
        assert!(is_entry_point("__init__"));
        assert!(is_entry_point("setup"));
        assert!(is_entry_point("teardown"));
        assert!(is_entry_point("setUp"));
        assert!(is_entry_point("tearDown"));
        assert!(is_entry_point("test_parse"));
        assert!(is_entry_point("parser_test"));
        assert!(is_entry_point("TestParse"));
        assert!(is_entry_point("test"));

        assert!(!is_entry_point("parse"));
        assert!(!is_entry_point("handle_request"));
        assert!(!is_entry_point("my_function"));
    }
}
