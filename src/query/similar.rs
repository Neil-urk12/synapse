use lbug::{Connection, LogicalType, QueryResult, Value};
use std::collections::HashMap;

use crate::embedder::Embedder;
use crate::query::error::QueryError;
use crate::types::query::SimilarChunk;

#[derive(Debug, Clone)]
pub struct ScoredChunk {
    pub id: String,
    pub text: String,
    pub language: String,
    pub score: f32,
}

/// Extract ScoredChunks from a vector query result, converting distance to similarity.
/// Filters by threshold — chunks below the threshold are excluded.
pub fn scored_chunks_from_result(result: QueryResult, threshold: f32) -> Vec<ScoredChunk> {
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
        #[allow(clippy::cast_precision_loss)]
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

/// Execute a semantic similarity search over indexed code chunks.
///
/// Embeds `query` using BGE-small-en-v1.5, executes vector similarity search against LadybugDB,
/// enriches the top chunks with their owning file path and start line via `DOCUMENTED_BY` edges,
/// and returns the structured chunks.
pub fn query_similar(
    conn: &Connection,
    query: &str,
    limit: usize,
    threshold: f32,
) -> Result<Vec<SimilarChunk>, QueryError> {
    let mut model = Embedder::try_new().map_err(|e| QueryError::Embedder(e.to_string()))?;
    let query_emb = model
        .embed(&[query.to_string()])
        .map_err(|e| QueryError::Embedder(e.to_string()))?;
    let query_vec = &query_emb[0];
    let vec_str: Vec<String> = query_vec.iter().map(|f| f.to_string()).collect();

    let search_query = format!(
        "CALL QUERY_VECTOR_INDEX('Chunk', 'idx_chunk_vector', [{}], {}) \
         YIELD node, distance RETURN node.id, node.text, node.language, distance",
        vec_str.join(", "),
        limit
    );
    let result = conn
        .query(&search_query)
        .map_err(|e| QueryError::Database(format!("vector query: {e}")))?;
    let scored = scored_chunks_from_result(result, threshold);

    if scored.is_empty() {
        // Distinguish "no embeddings exist" from "no matches above threshold".
        let count_res =
            conn.query("MATCH (c:Chunk) WHERE c.embedding IS NOT NULL RETURN count(c) AS n");
        let any_embedded = count_res
            .ok()
            .and_then(|mut r| r.next().and_then(|row| row.first().cloned()))
            .map(|v| v.to_string() != "0")
            .unwrap_or(false);
        if !any_embedded {
            return Err(QueryError::NotEmbedded(
                "run 'synapse embed' to populate embeddings".into(),
            ));
        }
        return Ok(Vec::new());
    }

    // Follow-up: pull file_path and start_line for each surviving chunk.
    let chunk_ids: Vec<Value> = scored.iter().map(|s| Value::String(s.id.clone())).collect();
    let id_list = Value::List(LogicalType::String, chunk_ids);
    let lookup_query = "MATCH (c:Chunk) WHERE c.id IN $ids \
                        OPTIONAL MATCH (file:File)-[:DOCUMENTED_BY]->(c) \
                        OPTIONAL MATCH (sym:Symbol)-[:DOCUMENTED_BY]->(c) \
                        RETURN c.id, file.path, sym.start_line";
    let mut lookup_stmt = conn
        .prepare(lookup_query)
        .map_err(|e| QueryError::Database(format!("prepare lookup: {e}")))?;
    let lookup_result = conn
        .execute(&mut lookup_stmt, vec![("ids", id_list)])
        .map_err(|e| QueryError::Database(format!("lookup query: {e}")))?;

    let mut location_by_chunk: HashMap<String, (String, i64)> = HashMap::new();
    for row in lookup_result {
        let id = match row.first() {
            Some(Value::String(s)) => s.clone(),
            _ => continue,
        };
        let file_path = match row.get(1) {
            Some(Value::String(p)) => p.clone(),
            _ => String::new(),
        };
        let start_line = match row.get(2) {
            Some(Value::Int64(n)) => *n,
            _ => 0,
        };
        location_by_chunk.insert(id, (file_path, start_line));
    }

    let chunks: Vec<SimilarChunk> = scored
        .into_iter()
        .map(|s| {
            let (file_path, start_line) = location_by_chunk
                .get(&s.id)
                .cloned()
                .unwrap_or((String::new(), 0));
            SimilarChunk {
                chunk_id: s.id,
                text: s.text,
                language: s.language,
                score: s.score,
                file_path,
                start_line,
            }
        })
        .collect();

    Ok(chunks)
}
