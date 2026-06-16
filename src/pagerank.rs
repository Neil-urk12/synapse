use lbug::{Connection, Value};
use std::collections::{HashMap, HashSet};

/// Power iteration PageRank over a Symbol→Symbol adjacency graph.
///
/// `out_edges[u]` is the set of nodes that node `u` has outbound edges to
/// (caller → callee). Self-loops are ignored (filtered by caller).
///
/// Returns a vector of length `n` whose values sum to ≈ 1.0.
/// If `n == 0`, returns an empty vector.
pub fn power_iterate(
    n: usize,
    out_edges: &HashMap<usize, Vec<usize>>,
    damping: f64,
    epsilon: f64,
    max_iters: usize,
) -> Vec<f64> {
    if n == 0 {
        return Vec::new();
    }

    // Initial state: uniform 1/N
    let mut scores: Vec<f64> = vec![1.0 / n as f64; n];
    let teleport = (1.0 - damping) / n as f64;

    for _ in 0..max_iters {
        // Dangling mass: sum of scores for nodes with no outbound edges.
        // Must be redistributed to all nodes, otherwise PageRank leaks mass
        // and the score vector no longer sums to 1.0.
        let dangling_mass: f64 = (0..n)
            .filter(|u| match out_edges.get(u) {
                Some(t) => t.is_empty(),
                None => true,
            })
            .map(|u| scores[u])
            .sum();
        let dangling_share = damping * dangling_mass / n as f64;

        let mut next = vec![teleport + dangling_share; n];

        // Distribute mass from non-dangling nodes
        for (&u, targets) in out_edges {
            if targets.is_empty() {
                continue;
            }
            let share = scores[u] / targets.len() as f64;
            for &v in targets {
                if v != u {
                    // ignore self-loops
                    next[v] += damping * share;
                }
            }
        }

        // L1 norm of delta → convergence check
        let delta: f64 = next
            .iter()
            .zip(scores.iter())
            .map(|(a, b)| (a - b).abs())
            .sum();
        scores = next;
        if delta < epsilon {
            break;
        }
    }

    scores
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f64, b: f64, eps: f64) -> bool {
        (a - b).abs() < eps
    }

    #[test]
    fn test_power_iterate_simple_chain() {
        // A → B → C  (caller → callee)
        let mut edges = HashMap::new();
        edges.insert(0, vec![1]); // A → B
        edges.insert(1, vec![2]); // B → C
        edges.insert(2, vec![]); // C is leaf
        let scores = power_iterate(3, &edges, 0.85, 1e-6, 100);
        assert!(scores[2] > scores[1], "C should score higher than B");
        assert!(scores[1] > scores[0], "B should score higher than A");
    }

    #[test]
    fn test_power_iterate_cycle() {
        // A ↔ B (mutual calls)
        let mut edges = HashMap::new();
        edges.insert(0, vec![1]);
        edges.insert(1, vec![0]);
        let scores = power_iterate(2, &edges, 0.85, 1e-6, 100);
        assert!(approx_eq(scores[0], scores[1], 1e-4));
    }

    #[test]
    fn test_power_iterate_star() {
        // A → {B, C, D}; B,C,D are leaves
        let mut edges = HashMap::new();
        edges.insert(0, vec![1, 2, 3]);
        edges.insert(1, vec![]);
        edges.insert(2, vec![]);
        edges.insert(3, vec![]);
        let scores = power_iterate(4, &edges, 0.85, 1e-6, 100);
        // B/C/D are called by A; A has no inbound. Hub B/C/D should outrank A.
        assert!(scores[1] > scores[0]);
        assert!(scores[2] > scores[0]);
        assert!(scores[3] > scores[0]);
    }

    #[test]
    fn test_power_iterate_dangling_node() {
        // A → B; C has no edges at all
        let mut edges = HashMap::new();
        edges.insert(0, vec![1]);
        edges.insert(1, vec![]);
        // C is not in the map (no entry)
        let scores = power_iterate(3, &edges, 0.85, 1e-6, 100);
        let sum: f64 = scores.iter().sum();
        assert!(
            approx_eq(sum, 1.0, 1e-4),
            "scores must sum to 1.0 (got {})",
            sum
        );
        // C should still receive the teleport mass
        assert!(scores[2] > 0.0);
    }

    #[test]
    fn test_power_iterate_convergence() {
        // 100-node random-ish graph
        let mut edges = HashMap::new();
        for i in 0..100 {
            let targets: Vec<usize> = ((i + 1)..(i + 4).min(100)).collect();
            edges.insert(i, targets);
        }
        let scores = power_iterate(100, &edges, 0.85, 1e-6, 100);
        // Should not panic / not return NaN
        assert!(scores.iter().all(|s| s.is_finite()));
        let sum: f64 = scores.iter().sum();
        assert!(approx_eq(sum, 1.0, 1e-4));
    }

    #[test]
    fn test_power_iterate_normalization() {
        // Triangle: A→B, B→C, C→A
        let mut edges = HashMap::new();
        edges.insert(0, vec![1]);
        edges.insert(1, vec![2]);
        edges.insert(2, vec![0]);
        let scores = power_iterate(3, &edges, 0.85, 1e-6, 100);
        let sum: f64 = scores.iter().sum();
        assert!(
            approx_eq(sum, 1.0, 1e-4),
            "expected sum=1.0, got {}",
            sum
        );
    }

    #[test]
    fn test_power_iterate_empty_graph() {
        let edges: HashMap<usize, Vec<usize>> = HashMap::new();
        let scores = power_iterate(0, &edges, 0.85, 1e-6, 100);
        assert!(scores.is_empty());
    }
}

/// Load CALLS edges from the DB, run PageRank, and persist scores onto Symbol.pagerank.
pub fn compute_and_store_pagerank(
    conn: &Connection,
    verbose: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if verbose {
        println!("📊 Computing PageRank over CALLS graph...");
    }

    // 1. Load all symbols (id → index)
    let mut id_to_idx: HashMap<String, usize> = HashMap::new();
    let mut idx_to_id: Vec<String> = Vec::new();
    let sym_query = conn.query("MATCH (s:Symbol) RETURN s.id")?;
    for row in sym_query {
        if let Some(Value::String(id)) = row.first() {
            id_to_idx.insert(id.clone(), idx_to_id.len());
            idx_to_id.push(id.clone());
        }
    }
    let n = idx_to_id.len();
    if n == 0 {
        if verbose {
            println!("⚠️  No symbols found, skipping PageRank.");
        }
        return Ok(());
    }

    // 2. Load CALLS edges → adjacency (use set to collapse parallel edges)
    let mut out_edges: HashMap<usize, HashSet<usize>> = HashMap::new();
    let call_query = conn.query("MATCH (a:Symbol)-[:CALLS]->(b:Symbol) RETURN a.id, b.id")?;
    for row in call_query {
        if let (Some(Value::String(from)), Some(Value::String(to))) = (row.first(), row.get(1)) {
            if let (Some(&u), Some(&v)) = (id_to_idx.get(from), id_to_idx.get(to)) {
                out_edges.entry(u).or_default().insert(v);
            }
        }
    }
    // Convert to Vec<Vec<usize>> for power_iterate
    let out_edges_vec: HashMap<usize, Vec<usize>> = out_edges
        .into_iter()
        .map(|(k, v)| (k, v.into_iter().collect()))
        .collect();

    if verbose {
        println!(
            "   {} symbols, {} caller nodes with edges",
            n,
            out_edges_vec.len()
        );
    }

    // 3. Run power iteration
    let scores = power_iterate(n, &out_edges_vec, 0.85, 1e-6, 100);
    let sum: f64 = scores.iter().sum();
    if verbose {
        println!(
            "   scores: min={:.6}, max={:.6}, sum={:.6}",
            scores.iter().cloned().fold(f64::INFINITY, f64::min),
            scores.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
            sum
        );
    }

    // 4. Persist scores via prepared statement
    let mut stmt = conn.prepare("MATCH (s:Symbol {id: $id}) SET s.pagerank = $score")?;
    for (i, score) in scores.iter().enumerate() {
        let params: Vec<(&str, Value)> = vec![
            ("id", Value::String(idx_to_id[i].clone())),
            ("score", Value::Double(*score)),
        ];
        conn.execute(&mut stmt, params)?;
    }

    if verbose {
        println!("✅ PageRank complete: {} symbols scored.", n);
    }
    Ok(())
}

#[cfg(test)]
mod io_tests {
    use super::*;
    use lbug::{Database, SystemConfig};

    #[test]
    fn test_compute_and_store_pagerank_empty_db() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test_pagerank.lbug");
        let db = Database::new(&db_path, SystemConfig::default()).unwrap();
        let conn = Connection::new(&db).unwrap();
        crate::schema::init_schema(&conn).unwrap();
        // No symbols → should be a no-op
        compute_and_store_pagerank(&conn, false).unwrap();
    }
}
