// CLI command — stdout is the output. Migrating to `tracing` is tracked
// in `docs/audits/2026-06-15-rust-best-practices-audit.md` Finding 12.
#![allow(clippy::print_stdout)]

use crate::embedder;
use lbug::{Connection, Database, SystemConfig};
use std::path::Path;

pub use crate::query::similar::{query_similar, scored_chunks_from_result, ScoredChunk};

/// Render scored chunks as a JSON string.
pub fn format_scored_json(scored: &[ScoredChunk]) -> Result<String, Box<dyn std::error::Error>> {
    let results: Vec<serde_json::Value> = scored
        .iter()
        .map(|s| {
            serde_json::json!({
                "chunk_id": s.id,
                "score": s.score,
                "language": s.language,
                "source_code": s.text,
            })
        })
        .collect();
    Ok(serde_json::to_string_pretty(&results)?)
}

/// Render scored chunks as a markdown string.
pub fn format_scored_markdown(scored: &[ScoredChunk], query: &str) -> String {
    let mut out = format!("# Similar to: \"{}\"\n\n", query);
    for s in scored {
        out.push_str(&format!("## {} (Score: {:.2})\n", s.id, s.score));
        let extension = s.language.to_lowercase();
        out.push_str(&format!("```{}\n{}\n```\n\n", extension, s.text));
    }
    out
}

pub fn handle_similar(
    query: &str,
    db_path: &Path,
    limit: usize,
    threshold: f32,
    format: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let db = Database::new(db_path, SystemConfig::default())?;
    let conn = Connection::new(&db)?;

    let mut model = embedder::Embedder::try_new()?;
    let query_emb = model.embed(&[query.to_string()])?;
    let query_vec = &query_emb[0];

    let vec_str = query_vec
        .iter()
        .map(|f| f.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let search_query = format!(
        "CALL QUERY_VECTOR_INDEX('Chunk', 'idx_chunk_vector', [{}], {}) YIELD node, distance RETURN node.id, node.text, node.language, distance",
        vec_str, limit
    );

    let result = conn.query(&search_query)?;
    let scored = scored_chunks_from_result(result, threshold);

    if scored.is_empty() {
        println!(
            "No similar chunks found. Try lowering --threshold or running `synapse embed` first."
        );
        return Ok(());
    }

    if format == "json" {
        println!("{}", format_scored_json(&scored)?);
    } else {
        print!("{}", format_scored_markdown(&scored, query));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_scored_json() {
        let chunks = vec![ScoredChunk {
            id: "src/main.rs::chunk::0".to_string(),
            text: "fn main() {}".to_string(),
            language: "Rust".to_string(),
            score: 0.92,
        }];
        let json = format_scored_json(&chunks).unwrap();
        assert!(json.contains("src/main.rs::chunk::0"));
        assert!(json.contains("0.92"));
        assert!(json.contains("chunk_id"));
        assert!(json.contains("score"));
    }

    #[test]
    fn test_format_scored_markdown() {
        let chunks = vec![ScoredChunk {
            id: "src/lib.rs::helper".to_string(),
            text: "pub fn helper() -> i32 { 42 }".to_string(),
            language: "Rust".to_string(),
            score: 0.85,
        }];
        let md = format_scored_markdown(&chunks, "helper function");
        assert!(md.contains("src/lib.rs::helper"));
        assert!(md.contains("0.85"));
        assert!(md.contains("```rust"));
        assert!(md.contains("helper function"));
    }
}
