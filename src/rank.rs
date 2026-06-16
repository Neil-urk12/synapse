use lbug::{Connection, Database, SystemConfig, Value};
use std::path::Path;

pub fn handle_rank(
    db_path: &Path,
    top: usize,
    kind: Option<&str>,
    format: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let db = Database::new(db_path, SystemConfig::default())?;
    let conn = Connection::new(&db)?;

    // Query symbols with non-null pagerank scores, optionally filtered by kind.
    // We sort in Rust (not via ORDER BY) to keep behavior consistent across
    // LadybugDB versions and to allow easy top-N truncation.
    let (query, params): (&str, Vec<(&str, Value)>) = match kind {
        Some(k) => (
            "MATCH (s:Symbol) WHERE s.pagerank IS NOT NULL AND s.kind = $kind \
             RETURN s.id, s.name, s.kind, s.pagerank",
            vec![("kind", Value::String(k.to_string()))],
        ),
        None => (
            "MATCH (s:Symbol) WHERE s.pagerank IS NOT NULL \
             RETURN s.id, s.name, s.kind, s.pagerank",
            vec![],
        ),
    };

    let mut stmt = conn.prepare(query)?;
    let result = conn.execute(&mut stmt, params)?;

    let mut rows: Vec<(String, String, String, f64)> = Vec::new();
    for row in result {
        if let (
            Some(Value::String(id)),
            Some(Value::String(name)),
            Some(Value::String(kind_val)),
            Some(Value::Double(score)),
        ) = (row.first(), row.get(1), row.get(2), row.get(3))
        {
            rows.push((id.clone(), name.clone(), kind_val.clone(), *score));
        }
    }

    if rows.is_empty() {
        eprintln!(
            "No symbols with PageRank scores found. \
             Run `synapse index` to populate scores."
        );
        return Ok(());
    }

    rows.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap_or(std::cmp::Ordering::Equal));
    rows.truncate(top);

    // Symbol.id format is "file_path::name" (top-level) or
    // "parent_id::name" (nested, where parent_id also starts with file_path).
    // The first segment before "::" is the file path — use it directly.
    let enriched: Vec<(usize, String, String, String, f64)> = rows
        .into_iter()
        .enumerate()
        .map(|(i, (id, name, kind, score))| {
            let file_path = id
                .split("::")
                .next()
                .unwrap_or(&id)
                .to_string();
            (i + 1, name, kind, file_path, score)
        })
        .collect();

    match format {
        "json" => print_json(&enriched),
        _ => print_markdown(&enriched),
    }

    Ok(())
}

fn print_markdown(rows: &[(usize, String, String, String, f64)]) {
    println!("# Top {} symbols by PageRank", rows.len());
    println!();
    println!("| Rank | Symbol | Kind | File | Score |");
    println!("|------|--------|------|------|-------|");
    for (rank, name, kind, file, score) in rows {
        println!(
            "| {} | `{}` | {} | `{}` | {:.4} |",
            rank, name, kind, file, score
        );
    }
}

fn print_json(rows: &[(usize, String, String, String, f64)]) {
    println!("[");
    for (i, (rank, name, kind, file, score)) in rows.iter().enumerate() {
        let comma = if i + 1 < rows.len() { "," } else { "" };
        println!(
            "  {{\"rank\":{},\"symbol\":{},\"kind\":{},\"file\":{},\"score\":{:.6}}}{}",
            rank, json_str(name), json_str(kind), json_str(file), score, comma
        );
    }
    println!("]");
}

fn json_str(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{}\"", escaped)
}
