use crate::query::callees::run_callees_internal;
use crate::query::callers::run_callers_internal;
use crate::query::context::run_context_internal;
use crate::query::dependencies::run_dependencies_internal;
use crate::query::raw::run_query_internal;
use lbug::{Connection, Database, SystemConfig};
use std::path::Path;

pub fn run_repl(db_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    use rustyline::error::ReadlineError;
    use rustyline::DefaultEditor;

    let db = Database::new(db_path, SystemConfig::default())?;
    let conn = Connection::new(&db)?;

    println!("==================================================");
    println!("⚡ Synapse Interactive Query Shell (REPL)");
    println!("Type '.help' for shortcuts, '.exit' to quit.");
    println!("==================================================");

    let mut rl = DefaultEditor::new()?;
    let history_path = db_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(".synapse_history");
    let _ = rl.load_history(&history_path);

    loop {
        let readline = rl.readline("synapse> ");
        match readline {
            Ok(line) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let _ = rl.add_history_entry(trimmed);
                match handle_repl_command(&conn, trimmed, &mut std::io::stdout()) {
                    Ok(should_continue) => {
                        if !should_continue {
                            break;
                        }
                    }
                    Err(e) => {
                        eprintln!("REPL Error: {}", e);
                    }
                }
            }
            Err(ReadlineError::Interrupted) => {
                println!("Ctrl+C");
                continue;
            }
            Err(ReadlineError::Eof) => {
                println!("Ctrl+D");
                break;
            }
            Err(err) => {
                eprintln!("Readline Error: {:?}", err);
                break;
            }
        }
    }

    let _ = rl.save_history(&history_path);
    println!("Exiting Synapse REPL.");
    Ok(())
}

pub fn handle_repl_command(
    conn: &Connection,
    line: &str,
    writer: &mut dyn std::io::Write,
) -> Result<bool, Box<dyn std::error::Error>> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(true);
    }

    if trimmed.starts_with('.') {
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        let cmd = parts[0];
        match cmd {
            ".exit" | ".quit" => return Ok(false),
            ".help" => {
                writeln!(writer, "Synapse REPL Shortcuts:")?;
                writeln!(writer, "  .help                    Show this help message")?;
                writeln!(
                    writer,
                    "  .callers <symbol>        Find all callers of a symbol"
                )?;
                writeln!(
                    writer,
                    "  .callees <symbol>        Find all callees of a symbol"
                )?;
                writeln!(
                    writer,
                    "  .deps <file>             List dependencies for a file"
                )?;
                writeln!(
                    writer,
                    "  .context <symbol/file>   Get code context (exact search)"
                )?;
                writeln!(writer, "  .exit / .quit            Exit the REPL")?;
            }
            ".callers" => {
                if parts.len() < 2 {
                    writeln!(
                        writer,
                        "Error: Missing target symbol. Usage: .callers <symbol>"
                    )?;
                } else {
                    let symbol = parts[1..].join(" ");
                    if let Err(e) = run_callers_internal(conn, &symbol, true, "table", writer) {
                        writeln!(writer, "Error running .callers: {}", e)?;
                    }
                }
            }
            ".callees" => {
                if parts.len() < 2 {
                    writeln!(
                        writer,
                        "Error: Missing target symbol. Usage: .callees <symbol>"
                    )?;
                } else {
                    let symbol = parts[1..].join(" ");
                    if let Err(e) = run_callees_internal(conn, &symbol, true, "table", writer) {
                        writeln!(writer, "Error running .callees: {}", e)?;
                    }
                }
            }
            ".deps" => {
                if parts.len() < 2 {
                    writeln!(writer, "Error: Missing target file. Usage: .deps <file>")?;
                } else {
                    let file = parts[1..].join(" ");
                    if let Err(e) = run_dependencies_internal(conn, &file, true, "table", writer) {
                        writeln!(writer, "Error running .deps: {}", e)?;
                    }
                }
            }
            ".context" => {
                if parts.len() < 2 {
                    writeln!(
                        writer,
                        "Error: Missing target. Usage: .context <symbol/file>"
                    )?;
                } else {
                    let target = parts[1..].join(" ");
                    if let Err(e) =
                        run_context_internal(conn, Some(&target), None, false, "markdown", writer)
                    {
                        if let Err(e2) = run_context_internal(
                            conn,
                            None,
                            Some(&target),
                            false,
                            "markdown",
                            writer,
                        ) {
                            writeln!(writer, "Error running .context (symbol): {}", e)?;
                            writeln!(writer, "Error running .context (file): {}", e2)?;
                        }
                    }
                }
            }
            _ => {
                writeln!(
                    writer,
                    "Error: Unknown shortcut command '{}'. Type '.help' for commands.",
                    cmd
                )?;
            }
        }
    } else {
        if let Err(e) = run_query_internal(conn, trimmed, writer) {
            writeln!(writer, "Error executing query: {}", e)?;
        }
    }

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lbug::{Connection, Database, SystemConfig};
    use std::path::Path;

    #[test]
    fn test_handle_repl_command() {
        let db_path = Path::new("test_repl_handler.lbug");
        if db_path.exists() {
            let _ = std::fs::remove_file(db_path);
        }
        let db = Database::new(db_path, SystemConfig::default()).unwrap();
        let conn = Connection::new(&db).unwrap();

        let mut buf = Vec::new();
        let should_continue = handle_repl_command(&conn, ".exit", &mut buf).unwrap();
        assert!(!should_continue);

        let mut buf = Vec::new();
        let should_continue = handle_repl_command(&conn, ".help", &mut buf).unwrap();
        assert!(should_continue);
        let out_str = String::from_utf8(buf).unwrap();
        assert!(out_str.contains("Synapse REPL Shortcuts:"));
        assert!(out_str.contains(".callers <symbol>"));

        let mut buf = Vec::new();
        let should_continue = handle_repl_command(&conn, ".invalid_cmd", &mut buf).unwrap();
        assert!(should_continue);
        let out_str = String::from_utf8(buf).unwrap();
        assert!(out_str.contains("Error: Unknown shortcut command"));

        let _ = std::fs::remove_file(db_path);
    }
}
