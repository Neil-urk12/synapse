//! Native vector extension lifecycle. Indexes must be dropped before chunk writes.
use crate::database::{self, DbResult};
use lbug::{Connection, Value};

pub const INDEX_NAME: &str = "idx_chunk_vector";

pub fn load(conn: &Connection) -> DbResult<()> {
    conn.query("LOAD vector").map_err(|e| format!("Cannot load LadybugDB vector extension: {e}. Run synapse embed -d DB to install the matching extension."))?;
    Ok(())
}

pub fn install(conn: &Connection) -> DbResult<()> {
    if load(conn).is_ok() {
        return Ok(());
    }
    conn.query("INSTALL vector")
        .map_err(|e| format!("Cannot install LadybugDB vector extension: {e}"))?;
    load(conn)
}

pub fn exists(conn: &Connection) -> DbResult<bool> {
    let rows = conn.query("CALL SHOW_INDEXES() RETURN table_name, index_name")?;
    Ok(rows.into_iter().any(|r| matches!((r.first(), r.get(1)), (Some(Value::String(table)), Some(Value::String(index))) if table == "Chunk" && index == INDEX_NAME)))
}

pub fn invalidate(conn: &Connection) -> DbResult<()> {
    database::set_dirty(conn, true, true)?;
    if exists(conn)? {
        load(conn)?;
        conn.query("CALL DROP_VECTOR_INDEX('Chunk', 'idx_chunk_vector')")?;
    }
    Ok(())
}

pub fn create(conn: &Connection) -> DbResult<()> {
    conn.query(
        "CALL CREATE_VECTOR_INDEX('Chunk', 'idx_chunk_vector', 'embedding', metric := 'cosine')",
    )?;
    if !exists(conn)? {
        return Err("Vector index creation did not produce idx_chunk_vector".into());
    }
    database::set_dirty(conn, true, false)
}

pub fn ensure_ready(conn: &Connection) -> DbResult<()> {
    if database::dirty(conn, true)? || !exists(conn)? {
        return Err("Semantic index is missing or stale; run synapse embed -d DB".into());
    }
    if conn
        .query("MATCH (c:Chunk) WHERE c.embedding IS NULL RETURN c.id LIMIT 1")?
        .next()
        .is_some()
    {
        return Err("Chunks need embeddings; run synapse embed -d DB".into());
    }
    load(conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lbug::{Database, LogicalType, SystemConfig};

    #[test]
    fn native_vector_index_survives_reopen_and_rebuilds_after_invalidation() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("vector.lbug");
        let embedding = |axis| {
            let mut values = vec![Value::Float(0.0); 384];
            values[axis] = Value::Float(1.0);
            Value::List(LogicalType::Float, values)
        };
        {
            let db = Database::new(&path, SystemConfig::default()).unwrap();
            let conn = Connection::new(&db).unwrap();
            database::initialize(&conn, tmp.path()).unwrap();
            install(&conn).unwrap();
            conn.query("CREATE (:File {path: 'x.rs'})").unwrap();
            let mut stmt = conn.prepare("CREATE (c:Chunk {id: $id, text: $id, language: 'Rust', start_line: 7, embedding: $embedding})").unwrap();
            for (id, axis) in [("a", 0), ("b", 1)] {
                conn.execute(
                    &mut stmt,
                    vec![
                        ("id", Value::String(id.into())),
                        ("embedding", embedding(axis)),
                    ],
                )
                .unwrap();
            }
            conn.query("MATCH (f:File), (c:Chunk) CREATE (f)-[:DOCUMENTED_BY]->(c)")
                .unwrap();
            create(&conn).unwrap();
            assert!(exists(&conn).unwrap());
        }
        {
            let db = Database::new(&path, SystemConfig::default().read_only(true)).unwrap();
            let conn = Connection::new(&db).unwrap();
            ensure_ready(&conn).unwrap();
            let mut query = vec![0.0; 384];
            query[0] = 1.0;
            let result = crate::query::similar::search_vector(&conn, &query, 2, 0.5).unwrap();
            assert_eq!(result.len(), 1);
            assert_eq!(result[0].chunk_id, "a");
            assert_eq!(result[0].file_path, "x.rs");
            assert_eq!(result[0].start_line, 7);
            assert!(result[0].score > 0.99);
            assert!(conn
                .query("MATCH (c:Chunk) SET c.text = 'changed'")
                .is_err());
        }
        {
            let db = Database::new(&path, SystemConfig::default()).unwrap();
            let conn = Connection::new(&db).unwrap();
            invalidate(&conn).unwrap();
            assert!(!exists(&conn).unwrap());
            assert!(ensure_ready(&conn).is_err());
            let mut stmt = conn
                .prepare("MATCH (c:Chunk {id: 'b'}) SET c.embedding = $embedding")
                .unwrap();
            conn.execute(&mut stmt, vec![("embedding", embedding(0))])
                .unwrap();
            load(&conn).unwrap();
            create(&conn).unwrap();
            ensure_ready(&conn).unwrap();
        }
    }
}
