use std::path::Path;
use lbug::{Connection, Database, SystemConfig};

pub fn run_query_internal(
    conn: &Connection,
    query: &str,
    writer: &mut dyn std::io::Write,
) -> Result<(), Box<dyn std::error::Error>> {
    let result = conn.query(query)?;
    let headers = result.get_column_names();
    let mut rows = Vec::new();

    for row in result {
        let row_strs: Vec<String> = row.iter().map(|val| val.to_string()).collect();
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

pub fn handle_context(
    symbol: Option<&str>,
    file: Option<&str>,
    fuzzy: bool,
    format: &str,
    db_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("Context: symbol={:?}, file={:?}, fuzzy={}, format={}", symbol, file, fuzzy, format);
    Ok(())
}

pub fn format_ascii_table(headers: &[String], rows: &[Vec<String>]) -> String {
    if headers.is_empty() {
        return String::new();
    }
    let mut col_widths = vec![0; headers.len()];
    for (i, h) in headers.iter().enumerate() {
        col_widths[i] = h.len();
    }
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < col_widths.len() && cell.len() > col_widths[i] {
                col_widths[i] = cell.len();
            }
        }
    }
    let mut separator = String::new();
    for w in &col_widths {
        separator.push('+');
        separator.push_str(&"-".repeat(*w + 2));
    }
    separator.push_str("+\n");

    let mut out = String::new();
    out.push_str(&separator);
    out.push('|');
    for (i, h) in headers.iter().enumerate() {
        out.push_str(&format!(" {:<width$} |", h, width = col_widths[i]));
    }
    out.push('\n');
    out.push_str(&separator);

    for row in rows {
        out.push('|');
        for (i, cell) in row.iter().enumerate() {
            out.push_str(&format!(" {:<width$} |", cell, width = col_widths[i]));
        }
        out.push('\n');
    }
    out.push_str(&separator.trim_end());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_table_rendering() {
        let headers = vec!["ID".to_string(), "NAME".to_string(), "VAL".to_string()];
        let rows = vec![
            vec!["1".to_string(), "Alice".to_string(), "10.5".to_string()],
            vec!["20".to_string(), "Bob".to_string(), "true".to_string()],
        ];
        let formatted = format_ascii_table(&headers, &rows);
        let expected = "+----+-------+------+\n\
                        | ID | NAME  | VAL  |\n\
                        +----+-------+------+\n\
                        | 1  | Alice | 10.5 |\n\
                        | 20 | Bob   | true |\n\
                        +----+-------+------+";
        assert_eq!(formatted.trim(), expected);
    }

    #[test]
    fn test_handle_query_integration() {
        let db_path = Path::new("test_query_cli.lbug");
        if db_path.exists() {
            let _ = std::fs::remove_file(db_path);
        }
        let db = Database::new(db_path, SystemConfig::default()).unwrap();
        let conn = Connection::new(&db).unwrap();
        conn.query("CREATE NODE TABLE T(id INT64, name STRING, PRIMARY KEY(id))").unwrap();
        conn.query("CREATE (:T {id: 101, name: 'Indexer'})").unwrap();

        let mut output_buf = Vec::new();
        run_query_internal(&conn, "MATCH (n:T) RETURN n.id AS ID, n.name AS NAME", &mut output_buf).unwrap();
        let output = String::from_utf8(output_buf).unwrap();

        assert!(output.contains("101"));
        assert!(output.contains("Indexer"));
        assert!(output.contains("ID"));
        assert!(output.contains("NAME"));
        let _ = std::fs::remove_file(db_path);
    }
}
