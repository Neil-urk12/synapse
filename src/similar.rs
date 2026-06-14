use crate::embedder;
use lbug::{Connection, Database, SystemConfig, Value};
use std::path::Path;

#[derive(Debug, Clone)]
pub struct ScoredChunk {
    pub id: String,
    pub text: String,
    pub language: String,
    pub score: f32,
}

/// Extract ScoredChunks from a query result, converting distance to similarity.
/// Filters by threshold — chunks below the threshold are excluded.
pub fn scored_chunks_from_result(result: lbug::QueryResult, threshold: f32) -> Vec<ScoredChunk> {
    let mut scored = Vec::new();
    for row in result {
        let id = match row.first() {
            Some(Value::String(s)) => s.clone(),
            _ => continue,
        };
        let text = match row.get(1) {
            Some(Value::String(s)) => s.clone(),
            _ => continue,
        };
        let language = match row.get(2) {
            Some(Value::String(s)) => s.clone(),
            _ => continue,
        };
        let distance = match row.get(3) {
            Some(Value::Float(f)) => *f,
            Some(Value::Int64(i)) => *i as f32,
            _ => continue,
        };
        let similarity = 1.0 - distance;
        if similarity >= threshold {
            scored.push(ScoredChunk {
                id,
                text,
                language,
                score: similarity,
            });
        }
    }
    scored
}

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
