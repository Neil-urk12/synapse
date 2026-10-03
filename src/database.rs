//! Index format, transaction, and repository ownership checks.
use lbug::{Connection, Value};
use std::error::Error;
use std::path::{Path, PathBuf};

pub type DbResult<T> = Result<T, Box<dyn Error>>;
pub const INDEX_VERSION: i64 = 1;
const REBUILD: &str =
    "Index format is outdated or incomplete; run synapse index PATH -d DB --rebuild";

#[derive(Debug, thiserror::Error)]
#[error("Database transaction failed ({operation}); rollback also failed ({rollback}). Stop using this database connection.")]
pub struct UnusableConnection {
    operation: String,
    rollback: String,
}

/// Commit all changes together, or roll them back and report the original error.
pub fn transaction<T>(conn: &Connection, work: impl FnOnce() -> DbResult<T>) -> DbResult<T> {
    conn.query("BEGIN TRANSACTION")?;
    let result = work().and_then(|value| {
        conn.query("COMMIT")?;
        Ok(value)
    });
    match result {
        Ok(value) => Ok(value),
        Err(error) => {
            if let Err(rollback) = conn.query("ROLLBACK") {
                // LadybugDB rolls back the active transaction itself on runtime errors.
                if rollback
                    .to_string()
                    .contains("No active transaction for ROLLBACK")
                {
                    return Err(error);
                }
                return Err(Box::new(UnusableConnection {
                    operation: error.to_string(),
                    rollback: rollback.to_string(),
                }));
            }
            Err(error)
        }
    }
}

pub fn initialize(conn: &Connection, root: &Path) -> DbResult<()> {
    if conn
        .query("MATCH (m:IndexMetadata) RETURN m.version")
        .is_ok()
    {
        let existing = repository_root(conn)?;
        if existing != root {
            return Err(format!(
                "Index belongs to '{}', not '{}'; use --rebuild to replace it",
                existing.display(),
                root.display()
            )
            .into());
        }
        return Ok(());
    }
    if conn.query("MATCH (f:File) RETURN count(f)").is_ok() {
        return Err(REBUILD.into());
    }
    crate::schema::init_schema(conn)?;
    let mut stmt = conn.prepare("CREATE (m:IndexMetadata {id: 'index', version: $version, repository_root: $root, graph_dirty: true, vector_dirty: true})")?;
    conn.execute(
        &mut stmt,
        vec![
            ("version", Value::Int64(INDEX_VERSION)),
            ("root", Value::String(root.to_string_lossy().into_owned())),
        ],
    )?;
    Ok(())
}

pub fn repository_root(conn: &Connection) -> DbResult<PathBuf> {
    let rows = conn
        .query("MATCH (m:IndexMetadata {id: 'index'}) RETURN m.version, m.repository_root")
        .map_err(|_| REBUILD)?;
    let row = rows.into_iter().next().ok_or(REBUILD)?;
    match (row.first(), row.get(1)) {
        (Some(Value::Int64(INDEX_VERSION)), Some(Value::String(root))) if !root.is_empty() => {
            Ok(PathBuf::from(root))
        }
        _ => Err(REBUILD.into()),
    }
}

pub fn dirty(conn: &Connection, vector: bool) -> DbResult<bool> {
    let property = if vector {
        "vector_dirty"
    } else {
        "graph_dirty"
    };
    let rows = conn.query(&format!(
        "MATCH (m:IndexMetadata {{id: 'index'}}) RETURN m.{property}"
    ))?;
    Ok(rows.into_iter().next().and_then(|r| r.into_iter().next()) != Some(Value::Bool(false)))
}

pub fn set_dirty(conn: &Connection, vector: bool, dirty: bool) -> DbResult<()> {
    let property = if vector {
        "vector_dirty"
    } else {
        "graph_dirty"
    };
    let mut stmt = conn.prepare(&format!(
        "MATCH (m:IndexMetadata {{id: 'index'}}) SET m.{property} = $dirty"
    ))?;
    conn.execute(&mut stmt, vec![("dirty", Value::Bool(dirty))])?;
    Ok(())
}
