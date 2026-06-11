use crate::types::query::{CalleeInfo, CallerInfo, ContainedSymbolInfo};
use lbug::{Connection, Value};

pub fn fetch_callers(
    conn: &Connection,
    target_id: &str,
) -> Result<Vec<CallerInfo>, Box<dyn std::error::Error>> {
    let mut stmt = conn.prepare(
        "MATCH (s1:Symbol)-[r:CALLS]->(s2:Symbol {id: $target_id}) \
         RETURN s1.id, s1.name, s1.kind, s1.signature, r.call_site_line",
    )?;
    let query_res = conn.execute(
        &mut stmt,
        vec![("target_id", Value::String(target_id.to_string()))],
    )?;
    let mut callers = Vec::new();
    for row in query_res {
        if let (
            Some(Value::String(id)),
            Some(Value::String(name)),
            Some(Value::String(kind)),
            Some(Value::String(sig)),
            Some(Value::Int64(line)),
        ) = (row.first(), row.get(1), row.get(2), row.get(3), row.get(4))
        {
            callers.push(CallerInfo {
                id: id.clone(),
                name: name.clone(),
                kind: kind.clone(),
                signature: sig.clone(),
                call_site_line: *line as usize,
            });
        }
    }
    Ok(callers)
}

pub fn fetch_callees(
    conn: &Connection,
    target_id: &str,
) -> Result<Vec<CalleeInfo>, Box<dyn std::error::Error>> {
    let mut stmt = conn.prepare(
        "MATCH (s1:Symbol {id: $target_id})-[r:CALLS]->(s2:Symbol) \
         RETURN s2.id, s2.name, s2.kind, s2.signature, r.call_site_line",
    )?;
    let query_res = conn.execute(
        &mut stmt,
        vec![("target_id", Value::String(target_id.to_string()))],
    )?;
    let mut callees = Vec::new();
    for row in query_res {
        if let (
            Some(Value::String(id)),
            Some(Value::String(name)),
            Some(Value::String(kind)),
            Some(Value::String(sig)),
            Some(Value::Int64(line)),
        ) = (row.first(), row.get(1), row.get(2), row.get(3), row.get(4))
        {
            callees.push(CalleeInfo {
                id: id.clone(),
                name: name.clone(),
                kind: kind.clone(),
                signature: sig.clone(),
                call_site_line: *line as usize,
            });
        }
    }
    Ok(callees)
}

pub fn fetch_imports(
    conn: &Connection,
    file_path: &str,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut stmt =
        conn.prepare("MATCH (f1:File {path: $path})-[:IMPORTS]->(f2:File) RETURN f2.path")?;
    let query_res = conn.execute(
        &mut stmt,
        vec![("path", Value::String(file_path.to_string()))],
    )?;
    let mut imports = Vec::new();
    for row in query_res {
        if let Some(Value::String(import_path)) = row.first() {
            imports.push(import_path.clone());
        }
    }
    Ok(imports)
}

pub fn fetch_imported_by(
    conn: &Connection,
    file_path: &str,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut stmt =
        conn.prepare("MATCH (f1:File)-[:IMPORTS]->(f2:File {path: $path}) RETURN f1.path")?;
    let query_res = conn.execute(
        &mut stmt,
        vec![("path", Value::String(file_path.to_string()))],
    )?;
    let mut imported_by = Vec::new();
    for row in query_res {
        if let Some(Value::String(imported_path)) = row.first() {
            imported_by.push(imported_path.clone());
        }
    }
    Ok(imported_by)
}

pub fn fetch_contained_symbols(
    conn: &Connection,
    file_path: &str,
) -> Result<Vec<ContainedSymbolInfo>, Box<dyn std::error::Error>> {
    let mut stmt = conn.prepare("MATCH (f:File {path: $path})-[:CONTAINS*1..]->(s:Symbol) RETURN s.id, s.name, s.kind, s.signature")?;
    let query_res = conn.execute(
        &mut stmt,
        vec![("path", Value::String(file_path.to_string()))],
    )?;
    let mut symbols = Vec::new();
    for row in query_res {
        if let (
            Some(Value::String(id)),
            Some(Value::String(name)),
            Some(Value::String(kind)),
            Some(Value::String(sig)),
        ) = (row.first(), row.get(1), row.get(2), row.get(3))
        {
            symbols.push(ContainedSymbolInfo {
                id: id.clone(),
                name: name.clone(),
                kind: kind.clone(),
                signature: sig.clone(),
            });
        }
    }
    Ok(symbols)
}
