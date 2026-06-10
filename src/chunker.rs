use crate::parser::NodeData;
use lbug::{Connection, Value};

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CodeChunk {
    pub id: String,
    pub text: String,
    pub symbol_id: Option<String>,
}

pub fn chunk_source(path: &str, content: &str, nodes: &[NodeData]) -> Vec<CodeChunk> {
    chunk_source_with_options(path, content, nodes, 30, 5)
}

pub fn chunk_source_with_options(
    path: &str,
    content: &str,
    nodes: &[NodeData],
    max_lines: usize,
    overlap_lines: usize,
) -> Vec<CodeChunk> {
    if !nodes.is_empty() {
        let mut chunks = Vec::new();
        for (idx, node) in nodes.iter().enumerate() {
            let text = crate::query_cli::slice_source_code(content, node.start_line, node.end_line);
            chunks.push(CodeChunk {
                id: format!("{}::chunk::{}", path, idx),
                text,
                symbol_id: Some(node.id.clone()),
            });
        }
        return chunks;
    }

    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return Vec::new();
    }

    let mut chunks = Vec::new();
    let mut start = 0;
    let mut idx = 0;

    while start < lines.len() {
        let end = std::cmp::min(start + max_lines, lines.len());
        let chunk_lines = &lines[start..end];
        let text = chunk_lines.join("\n");

        chunks.push(CodeChunk {
            id: format!("{}::chunk::{}", path, idx),
            text,
            symbol_id: None,
        });

        idx += 1;

        if end == lines.len() {
            break;
        }

        let step = if max_lines > overlap_lines {
            max_lines - overlap_lines
        } else {
            1
        };

        if start + step >= lines.len() {
            break;
        }
        start += step;
    }

    chunks
}

pub fn insert_chunks(
    conn: &Connection,
    file_path: &str,
    language: &str,
    chunks: &[CodeChunk],
) -> Result<(), Box<dyn std::error::Error>> {
    // 1. Cleanup old chunks
    let mut prepared_delete_file_chunks =
        conn.prepare("MATCH (f:File {path: $path})-[:DOCUMENTED_BY]->(c:Chunk) DETACH DELETE c")?;
    conn.execute(
        &mut prepared_delete_file_chunks,
        vec![("path", Value::String(file_path.to_string()))],
    )?;

    let mut prepared_delete_symbol_chunks = conn.prepare(
        "MATCH (f:File {path: $path})-[:CONTAINS*1..]->(s:Symbol)-[:DOCUMENTED_BY]->(c:Chunk) DETACH DELETE c"
    )?;
    conn.execute(
        &mut prepared_delete_symbol_chunks,
        vec![("path", Value::String(file_path.to_string()))],
    )?;

    // 2. Insert new chunks
    let mut prepared_chunk = conn.prepare(
        "MERGE (c:Chunk {id: $id}) \
         ON CREATE SET c.text = $text, c.language = $language, c.embedding = $embedding \
         ON MATCH SET c.text = $text, c.language = $language, c.embedding = $embedding",
    )?;

    let mut prepared_symbol_rel = conn.prepare(
        "MATCH (s:Symbol {id: $from_id}), (c:Chunk {id: $to_id}) \
         CREATE (s)-[:DOCUMENTED_BY]->(c)",
    )?;

    let mut prepared_file_rel = conn.prepare(
        "MATCH (f:File {path: $from_id}), (c:Chunk {id: $to_id}) \
         CREATE (f)-[:DOCUMENTED_BY]->(c)",
    )?;

    let zero_embedding = Value::List(lbug::LogicalType::Float, vec![Value::Float(0.0); 384]);

    for chunk in chunks {
        let chunk_params = vec![
            ("id", Value::String(chunk.id.clone())),
            ("text", Value::String(chunk.text.clone())),
            ("language", Value::String(language.to_string())),
            ("embedding", zero_embedding.clone()),
        ];
        conn.execute(&mut prepared_chunk, chunk_params)?;

        if let Some(ref sym_id) = chunk.symbol_id {
            let rel_params = vec![
                ("from_id", Value::String(sym_id.clone())),
                ("to_id", Value::String(chunk.id.clone())),
            ];
            conn.execute(&mut prepared_symbol_rel, rel_params)?;
        } else {
            let rel_params = vec![
                ("from_id", Value::String(file_path.to_string())),
                ("to_id", Value::String(chunk.id.clone())),
            ];
            conn.execute(&mut prepared_file_rel, rel_params)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lbug::{Connection, Database, SystemConfig};
    use std::path::Path;

    #[test]
    fn test_chunk_db_insertion() {
        let db_path = Path::new("test_chunk_db.lbug");
        if db_path.exists() {
            let _ = std::fs::remove_file(db_path);
        }

        let db = Database::new(db_path, SystemConfig::default()).unwrap();
        let conn = Connection::new(&db).unwrap();

        // Initialize schema (including Chunk and DOCUMENTED_BY)
        conn.query("CREATE NODE TABLE File (path STRING, language STRING, file_size INT64, hash STRING, raw_imports STRING, PRIMARY KEY (path))").unwrap();
        conn.query("CREATE NODE TABLE Symbol (id STRING, name STRING, kind STRING, start_line INT64, start_col INT64, end_line INT64, signature STRING, raw_calls STRING, PRIMARY KEY (id))").unwrap();
        conn.query("CREATE NODE TABLE Chunk (id STRING, text STRING, language STRING, embedding FLOAT[384], PRIMARY KEY (id))").unwrap();
        conn.query("CREATE REL TABLE DOCUMENTED_BY (FROM File TO Chunk, FROM Symbol TO Chunk)")
            .unwrap();
        conn.query("CREATE REL TABLE CONTAINS (FROM File TO Symbol, FROM Symbol TO Symbol)")
            .unwrap();

        // 1. Insert file and symbol
        let mut file_stmt = conn.prepare("CREATE (f:File {path: 'src/main.rs', language: 'Rust', file_size: 100, hash: 'abc', raw_imports: '[]'})").unwrap();
        conn.execute(&mut file_stmt, vec![]).unwrap();

        let mut sym_stmt = conn.prepare("CREATE (s:Symbol {id: 'src/main.rs::main', name: 'main', kind: 'Function', start_line: 1, start_col: 1, end_line: 10, signature: 'fn main()', raw_calls: '[]'})").unwrap();
        conn.execute(&mut sym_stmt, vec![]).unwrap();

        // 2. Insert dummy chunks using chunker
        let chunks = vec![
            CodeChunk {
                id: "src/main.rs::chunk::0".to_string(),
                text: "fn main() {\n}".to_string(),
                symbol_id: Some("src/main.rs::main".to_string()),
            },
            CodeChunk {
                id: "src/main.rs::chunk::1".to_string(),
                text: "// comment".to_string(),
                symbol_id: None,
            },
        ];

        insert_chunks(&conn, "src/main.rs", "Rust", &chunks).unwrap();

        // 3. Verify chunks were inserted
        let chunk_query = conn.query("MATCH (c:Chunk) RETURN c.id, c.text").unwrap();
        let mut chunks_found = Vec::new();
        for row in chunk_query {
            if let (Some(Value::String(id)), Some(Value::String(text))) = (row.first(), row.get(1))
            {
                chunks_found.push((id.clone(), text.clone()));
            }
        }
        assert_eq!(chunks_found.len(), 2);

        // 4. Verify DOCUMENTED_BY edges exist
        let rel_query = conn
            .query("MATCH (s:Symbol)-[:DOCUMENTED_BY]->(c:Chunk) RETURN s.id, c.id")
            .unwrap();
        let mut symbol_rel_found = false;
        for row in rel_query {
            if let (Some(Value::String(s_id)), Some(Value::String(c_id))) =
                (row.first(), row.get(1))
            {
                if s_id == "src/main.rs::main" && c_id == "src/main.rs::chunk::0" {
                    symbol_rel_found = true;
                }
            }
        }
        assert!(
            symbol_rel_found,
            "DOCUMENTED_BY relationship between Symbol and Chunk was not created"
        );

        let file_rel_query = conn
            .query("MATCH (f:File)-[:DOCUMENTED_BY]->(c:Chunk) RETURN f.path, c.id")
            .unwrap();
        let mut file_rel_found = false;
        for row in file_rel_query {
            if let (Some(Value::String(f_path)), Some(Value::String(c_id))) =
                (row.first(), row.get(1))
            {
                if f_path == "src/main.rs" && c_id == "src/main.rs::chunk::1" {
                    file_rel_found = true;
                }
            }
        }
        assert!(
            file_rel_found,
            "DOCUMENTED_BY relationship between File and Chunk was not created"
        );

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn test_chunk_source() {
        let content = "fn hello() {\n    println!(\"Hello!\");\n}\n\nstruct World;\n";
        let nodes = vec![
            NodeData {
                id: "src/main.rs::hello".to_string(),
                name: "hello".to_string(),
                kind: "Function".to_string(),
                start_line: 1,
                start_col: 1,
                end_line: 3,
                signature: "fn hello()".to_string(),
            },
            NodeData {
                id: "src/main.rs::World".to_string(),
                name: "World".to_string(),
                kind: "Struct".to_string(),
                start_line: 5,
                start_col: 1,
                end_line: 5,
                signature: "struct World;".to_string(),
            },
        ];

        let chunks = chunk_source("src/main.rs", content, &nodes);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].symbol_id, Some("src/main.rs::hello".to_string()));
        assert_eq!(chunks[0].text, "fn hello() {\n    println!(\"Hello!\");\n}");
        assert_eq!(chunks[1].symbol_id, Some("src/main.rs::World".to_string()));
        assert_eq!(chunks[1].text, "struct World;");
    }

    #[test]
    fn test_chunk_source_no_symbols() {
        let content = "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10";
        let chunks = chunk_source("src/utils.rs", content, &[]);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].symbol_id, None);
        assert_eq!(chunks[0].text, content);
    }

    #[test]
    fn test_chunk_source_sliding_window() {
        let content = "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10";
        // Call with custom options: max_lines = 4, overlap = 1
        let chunks = chunk_source_with_options("src/utils.rs", content, &[], 4, 1);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].text, "line1\nline2\nline3\nline4");
        assert_eq!(chunks[1].text, "line4\nline5\nline6\nline7");
        assert_eq!(chunks[2].text, "line7\nline8\nline9\nline10");
    }
}
