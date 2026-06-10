use crate::embedder;
use lbug::{Connection, Database, SystemConfig, Value};
use std::path::Path;

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

    struct ScoredChunk {
        id: String,
        text: String,
        language: String,
        score: f32,
    }

    let mut scored: Vec<ScoredChunk> = Vec::new();

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

    if scored.is_empty() {
        println!(
            "No similar chunks found. Try lowering --threshold or running `synapse embed` first."
        );
        return Ok(());
    }

    if format == "json" {
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
        println!("{}", serde_json::to_string_pretty(&results)?);
    } else {
        println!("# Similar to: \"{}\"\n", query);
        for s in &scored {
            println!("## {} (Score: {:.2})", s.id, s.score);
            let extension = s.language.to_lowercase();
            println!("```{}\n{}\n```\n", extension, s.text);
        }
    }

    Ok(())
}
