use lbug::Connection;

/// All DDL statements for Synapse graph schema.
pub fn schema_ddls() -> Vec<&'static str> {
    vec![
        "CREATE NODE TABLE File (path STRING, language STRING, file_size INT64, hash STRING, raw_imports STRING, PRIMARY KEY (path))",
        "CREATE NODE TABLE Symbol (id STRING, name STRING, kind STRING, start_line INT64, start_col INT64, end_line INT64, signature STRING, raw_calls STRING, pagerank DOUBLE, PRIMARY KEY (id))",
        "CREATE NODE TABLE Chunk (id STRING, text STRING, language STRING, embedding FLOAT[384], PRIMARY KEY (id))",
        "CREATE REL TABLE CONTAINS (FROM File TO Symbol, FROM Symbol TO Symbol)",
        "CREATE REL TABLE IMPORTS (FROM File TO File)",
        "CREATE REL TABLE CALLS (FROM Symbol TO Symbol, call_site_line INT64)",
        "CREATE REL TABLE DOCUMENTED_BY (FROM File TO Chunk, FROM Symbol TO Chunk)",
        // NOTE: LadybugDB 0.17 does not support `ALTER TABLE ... ADD COLUMN`,
        // so the `pagerank` column cannot be added in-place to legacy DBs.
        // Users with a pre-0.x DB must delete the DB file (or run a fresh
        // `synapse index` in a clean location). Fresh DBs get the column
        // from the CREATE NODE TABLE above.
    ]
}

/// Initialize schema tables. "already exists" errors silently skipped.
pub fn init_schema(conn: &Connection) -> Result<(), Box<dyn std::error::Error>> {
    for ddl in schema_ddls() {
        if let Err(e) = conn.query(ddl) {
            let err_msg = e.to_string().to_lowercase();
            if !err_msg.contains("already exists") && !err_msg.contains("duplicate") {
                return Err(Box::new(e));
            }
        }
    }
    Ok(())
}
