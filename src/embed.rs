use lbug::{Connection, Database, SystemConfig, Value};
use std::path::Path;

use crate::embedder;
use crate::schema;

pub fn handle_embed(
    db_path: &Path,
    batch_size: usize,
    verbose: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let db = Database::new(db_path, SystemConfig::default())?;
    let conn = Connection::new(&db)?;

    schema::init_schema(&conn)?;

    let index_ddl = "CREATE VECTOR INDEX idx_chunk_vector ON Chunk(embedding) USING HNSW WITH (metric = 'cosine', m = 16, ef_construction = 200, ef_search = 100)";
    if let Err(e) = conn.query(index_ddl) {
        let err_msg = e.to_string().to_lowercase();
        if !err_msg.contains("already exists") && !err_msg.contains("duplicate") {
            eprintln!("Warning: Failed to create vector index: {}", e);
        }
    }

    let count_query = "MATCH (c:Chunk) RETURN count(c)";
    let total_chunks: usize = {
        let count_result = conn.query(count_query)?;
        count_result
            .into_iter()
            .next()
            .and_then(|row| row.into_iter().next())
            .and_then(|v| {
                if let Value::Int64(n) = v {
                    Some(n as usize)
                } else {
                    None
                }
            })
            .unwrap_or(0)
    };

    if total_chunks == 0 {
        println!("No chunks to embed. Database has no chunk nodes.");
        return Ok(());
    }

    if verbose {
        println!("🧠 Embedding chunks...");
        println!("Model: BGE-small-en-v1.5 (384-dim)");
        println!("Chunks in DB: {}", total_chunks);
    }

    let mut model = embedder::Embedder::try_new()?;
    let pb = if verbose {
        let pb = indicatif::ProgressBar::new(total_chunks as u64);
        pb.set_style(
            indicatif::ProgressStyle::default_bar()
                .template("{bar:40} {pos}/{len} ({eta})")
                .unwrap()
                .progress_chars("█▉▊▋▌▍▎▏ "),
        );
        Some(pb)
    } else {
        None
    };

    let mut stmt = conn.prepare("MATCH (c:Chunk) RETURN c.id, c.text, c.embedding")?;
    let result = conn.execute(&mut stmt, vec![])?;

    let mut update_stmt = conn.prepare("MATCH (c:Chunk {id: $id}) SET c.embedding = $embedding")?;

    let mut batch_ids: Vec<String> = Vec::with_capacity(batch_size);
    let mut batch_texts: Vec<String> = Vec::with_capacity(batch_size);
    let mut embedded = 0usize;
    let mut skipped = 0usize;

    for row in result {
        let (id, text, needs_embed) = match (row.first(), row.get(1), row.get(2)) {
            (Some(Value::String(id)), Some(Value::String(text)), embedding_val) => {
                let needs = match embedding_val {
                    Some(Value::List(_, ref items)) => {
                        let embedding_vec = items
                            .iter()
                            .map(|v| if let Value::Float(f) = v { *f } else { 0.0 })
                            .collect::<Vec<f32>>();
                        embedder::is_zero_vector(&embedding_vec)
                    }
                    _ => true,
                };
                (id.clone(), text.clone(), needs)
            }
            _ => continue,
        };

        if needs_embed {
            batch_ids.push(id);
            batch_texts.push(text);
        } else {
            skipped += 1;
            if let Some(ref pb) = pb {
                pb.set_position((embedded + skipped) as u64);
            }
        }

        if batch_ids.len() >= batch_size {
            // Inline flush to avoid borrowed-local lifetimes
            let embeddings = model.embed(&batch_texts)?;
            for (i, emb) in embeddings.iter().enumerate() {
                let lbug_list = Value::List(
                    lbug::LogicalType::Float,
                    emb.iter().map(|&f| Value::Float(f)).collect(),
                );
                conn.execute(
                    &mut update_stmt,
                    vec![
                        ("id", Value::String(batch_ids[i].clone())),
                        ("embedding", lbug_list),
                    ],
                )?;
                embedded += 1;
            }
            if let Some(ref pb) = pb {
                pb.set_position((embedded + skipped) as u64);
            }
            batch_ids.clear();
            batch_texts.clear();
        }
    }

    // Final flush
    if !batch_ids.is_empty() {
        let embeddings = model.embed(&batch_texts)?;
        for (i, emb) in embeddings.iter().enumerate() {
            let lbug_list = Value::List(
                lbug::LogicalType::Float,
                emb.iter().map(|&f| Value::Float(f)).collect(),
            );
            conn.execute(
                &mut update_stmt,
                vec![
                    ("id", Value::String(batch_ids[i].clone())),
                    ("embedding", lbug_list),
                ],
            )?;
            embedded += 1;
        }
        if let Some(ref pb) = pb {
            pb.set_position((embedded + skipped) as u64);
        }
        batch_ids.clear();
        batch_texts.clear();
    }

    if let Some(pb) = pb {
        pb.finish_with_message("Done");
    }

    if embedded == 0 {
        println!(
            "No chunks to embed. All {} chunks already have embeddings.",
            skipped
        );
    } else {
        println!(
            "✅ Embedding complete. {} chunks embedded ({} skipped, already had embeddings).",
            embedded, skipped
        );
    }
    Ok(())
}
