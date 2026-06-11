use crate::query::format::format_ascii_table;
use lbug::{Connection, Database, SystemConfig};
use std::path::Path;

pub fn run_query_internal(
    conn: &Connection,
    query: &str,
    writer: &mut dyn std::io::Write,
) -> Result<(), Box<dyn std::error::Error>> {
    let result = conn.query(query)?;
    let headers = result.get_column_names();
    let mut rows = Vec::new();

    for row in result {
        let row_strs: Vec<String> = row
            .iter()
            .map(|val| val.to_string().replace('\n', "\\n").replace('\r', "\\r"))
            .collect();
        rows.push(row_strs);
    }

    let formatted_table = format_ascii_table(&headers, &rows);
    writeln!(writer, "{}", formatted_table)?;
    Ok(())
}

pub fn handle_query(query: &str, db_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let db = Database::new(db_path, SystemConfig::default())?;
    let conn = Connection::new(&db)?;
    run_query_internal(&conn, query, &mut std::io::stdout())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lbug::{Connection, Database, SystemConfig};
    use std::path::Path;

    #[test]
    fn test_handle_query_integration() {
        let db_path = Path::new("test_query_cli.lbug");
        if db_path.exists() {
            let _ = std::fs::remove_file(db_path);
        }
        let db = Database::new(db_path, SystemConfig::default()).unwrap();
        let conn = Connection::new(&db).unwrap();
        conn.query(
            "CREATE NODE TABLE T(id INT64, name STRING, PRIMARY KEY(id))",
        )
        .unwrap();
        conn.query("CREATE (:T {id: 101, name: 'Indexer'})")
            .unwrap();

        let mut output_buf = Vec::new();
        run_query_internal(
            &conn,
            "MATCH (n:T) RETURN n.id AS ID, n.name AS NAME",
            &mut output_buf,
        )
        .unwrap();
        let output = String::from_utf8(output_buf).unwrap();

        assert!(output.contains("101"));
        assert!(output.contains("Indexer"));
        assert!(output.contains("ID"));
        assert!(output.contains("NAME"));
        let _ = std::fs::remove_file(db_path);
    }
}
