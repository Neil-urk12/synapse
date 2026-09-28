use lbug::{Connection, Value};

use crate::query::error::QueryError;
use crate::types::query::RankItem;

/// Query symbols with non-null PageRank scores, optionally filtered by kind.
///
/// Results are sorted descending by PageRank score and truncated to `top`.
pub fn query_rank(
    conn: &Connection,
    top: usize,
    kind: Option<&str>,
) -> Result<Vec<RankItem>, QueryError> {
    let (query, params): (&str, Vec<(&str, Value)>) = match kind {
        Some(k) => (
            "MATCH (s:Symbol) WHERE s.pagerank IS NOT NULL AND s.kind = $kind \
             RETURN s.id, s.name, s.kind, s.pagerank",
            vec![("kind", Value::String(k.to_string()))],
        ),
        None => (
            "MATCH (s:Symbol) WHERE s.pagerank IS NOT NULL \
             RETURN s.id, s.name, s.kind, s.pagerank",
            vec![],
        ),
    };

    let mut stmt = conn.prepare(query)?;
    let result = conn.execute(&mut stmt, params)?;

    let mut rows: Vec<(String, String, String, f64)> = Vec::new();
    for row in result {
        if let (
            Some(Value::String(id)),
            Some(Value::String(name)),
            Some(Value::String(kind_val)),
            Some(Value::Double(score)),
        ) = (row.first(), row.get(1), row.get(2), row.get(3))
        {
            rows.push((id.clone(), name.clone(), kind_val.clone(), *score));
        }
    }

    rows.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap_or(std::cmp::Ordering::Equal));
    rows.truncate(top);

    let items: Vec<RankItem> = rows
        .into_iter()
        .enumerate()
        .map(|(i, (id, name, kind_name, score))| {
            let file_path = id.split("::").next().unwrap_or(&id).to_string();
            RankItem {
                rank: i + 1,
                symbol: name,
                kind: kind_name,
                file: file_path,
                score,
            }
        })
        .collect();

    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lbug::{Database, SystemConfig};
    use tempfile::tempdir;

    #[test]
    fn test_query_rank_empty_db() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.lbug");
        let db = Database::new(&db_path, SystemConfig::default()).unwrap();
        let conn = Connection::new(&db).unwrap();
        crate::schema::init_schema(&conn).unwrap();

        let ranked = query_rank(&conn, 10, None).unwrap();
        assert!(ranked.is_empty());
    }

    #[test]
    fn test_query_rank_sorting_and_filtering() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.lbug");
        let db = Database::new(&db_path, SystemConfig::default()).unwrap();
        let conn = Connection::new(&db).unwrap();
        crate::schema::init_schema(&conn).unwrap();

        conn.query("CREATE (:Symbol {id: 'f1::s1', name: 's1', kind: 'Function', start_line: 1, start_col: 1, end_line: 2, signature: 'fn s1()', raw_calls: '[]', pagerank: 0.1})").unwrap();
        conn.query("CREATE (:Symbol {id: 'f1::s2', name: 's2', kind: 'Function', start_line: 3, start_col: 1, end_line: 4, signature: 'fn s2()', raw_calls: '[]', pagerank: 0.8})").unwrap();
        conn.query("CREATE (:Symbol {id: 'f2::s3', name: 's3', kind: 'Method', start_line: 5, start_col: 1, end_line: 6, signature: 'fn s3()', raw_calls: '[]', pagerank: 0.5})").unwrap();

        let all = query_rank(&conn, 10, None).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].symbol, "s2");
        assert_eq!(all[0].rank, 1);
        assert_eq!(all[1].symbol, "s3");
        assert_eq!(all[1].rank, 2);
        assert_eq!(all[2].symbol, "s1");
        assert_eq!(all[2].rank, 3);

        let methods = query_rank(&conn, 10, Some("Method")).unwrap();
        assert_eq!(methods.len(), 1);
        assert_eq!(methods[0].symbol, "s3");
        assert_eq!(methods[0].file, "f2");
    }
}
