pub mod parser;

use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use ignore::WalkBuilder;
use lbug::{Connection, Database, SystemConfig, Value};
use sha2::{Digest, Sha256};

#[derive(Parser, Debug)]
#[command(name = "synapse")]
#[command(author, version, about = "Synapse: A local-first high-performance code intelligence graph database and indexer", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Index a codebase directory and construct its graph topology
    Index {
        /// Path to the codebase directory to index
        #[arg(default_value = ".")]
        path: PathBuf,

        /// Path to the LadybugDB database storage file
        #[arg(short, long, default_value = "synapse.lbug")]
        db: PathBuf,

        /// Enable verbose logging output
        #[arg(short, long)]
        verbose: bool,
    },
}

/// Computes the SHA-256 hash of a target file for incremental indexing detection.
fn compute_sha256(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 8192];

    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }

    Ok(hex::encode(hasher.finalize()))
}

/// Detects the programming language of a file based on its extension.
fn detect_language(path: &Path) -> String {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("rs") => "Rust".to_string(),
        Some("js") | Some("jsx") => "JavaScript".to_string(),
        Some("ts") | Some("tsx") => "TypeScript".to_string(),
        Some("py") => "Python".to_string(),
        Some("go") => "Go".to_string(),
        Some("cpp") | Some("cc") | Some("h") | Some("hpp") => "C++".to_string(),
        Some("c") => "C".to_string(),
        Some("java") => "Java".to_string(),
        Some("html") => "HTML".to_string(),
        Some("css") => "CSS".to_string(),
        Some("md") => "Markdown".to_string(),
        Some("json") => "JSON".to_string(),
        _ => "Unknown".to_string(),
    }
}

/// Initializes schema tables in LadybugDB. If tables already exist, errors are safely skipped.
fn init_schema(conn: &Connection) -> Result<(), Box<dyn std::error::Error>> {
    let ddls = vec![
        // Node Tables
        "CREATE NODE TABLE File (path STRING, language STRING, file_size INT64, hash STRING, PRIMARY KEY (path))",
        "CREATE NODE TABLE Symbol (id STRING, name STRING, kind STRING, start_line INT64, start_col INT64, end_line INT64, signature STRING, PRIMARY KEY (id))",
        "CREATE NODE TABLE Chunk (id STRING, text STRING, embedding FLOAT[384], PRIMARY KEY (id))",
        // Relationship Tables
        "CREATE REL TABLE CONTAINS (FROM File TO Symbol, FROM Symbol TO Symbol)",
        "CREATE REL TABLE IMPORTS (FROM File TO File)",
        "CREATE REL TABLE CALLS (FROM Symbol TO Symbol, call_site_line INT64)",
        "CREATE REL TABLE DOCUMENTED_BY (FROM File TO Chunk, FROM Symbol TO Chunk)"
    ];

    for ddl in ddls {
        if let Err(e) = conn.query(ddl) {
            let err_msg = e.to_string().to_lowercase();
            // Skip table creation failures due to table already existing in the database
            if !err_msg.contains("already exists") && !err_msg.contains("duplicate") {
                return Err(Box::new(e));
            }
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Index {
            path,
            db: db_path,
            verbose,
        } => {
            println!("==================================================");
            println!("⚡ Synapse Indexer Initializing");
            println!("==================================================");
            println!("Workspace Target : {}", path.display());
            println!("Database Target  : {}", db_path.display());
            println!("--------------------------------------------------");

            if !path.exists() {
                eprintln!(
                    "Error: Target workspace path '{}' does not exist.",
                    path.display()
                );
                std::process::exit(1);
            }

            // Ensure parent directory for database exists
            if let Some(parent) = db_path.parent() {
                if !parent.as_os_str().is_empty() && !parent.exists() {
                    if let Err(err) = std::fs::create_dir_all(parent) {
                        eprintln!("Error: Failed to create database path directory: {}", err);
                        std::process::exit(1);
                    }
                }
            }

            // 1. Initialize Database
            println!("📦 Connecting to LadybugDB...");
            let db = match Database::new(&db_path, SystemConfig::default()) {
                Ok(database) => database,
                Err(err) => {
                    eprintln!(
                        "Error: Failed to connect to LadybugDB at '{}': {}",
                        db_path.display(),
                        err
                    );
                    std::process::exit(1);
                }
            };

            // 2. Establish Connection
            let conn = match Connection::new(&db) {
                Ok(connection) => connection,
                Err(err) => {
                    eprintln!("Error: Failed to open database connection: {}", err);
                    std::process::exit(1);
                }
            };

            // 3. Initialize Tables / Verify Schema
            println!("🛠️  Verifying graph database schema...");
            if let Err(err) = init_schema(&conn) {
                eprintln!("Error: Failed to verify schema tables: {}", err);
                std::process::exit(1);
            }

            // 4. Prepare Statements
            let mut prepared_file_upsert = match conn.prepare(
                "MERGE (f:File {path: $path}) \
                 ON CREATE SET f.language = $language, f.file_size = $file_size, f.hash = $hash \
                 ON MATCH SET f.language = $language, f.file_size = $file_size, f.hash = $hash",
            ) {
                Ok(stmt) => stmt,
                Err(err) => {
                    eprintln!("Error: Failed to prepare File upsert query: {}", err);
                    std::process::exit(1);
                }
            };

            let mut prepared_check_hash = match conn.prepare(
                "MATCH (f:File {path: $path}) RETURN f.hash"
            ) {
                Ok(stmt) => stmt,
                Err(err) => {
                    eprintln!("Error: Failed to prepare hash check query: {}", err);
                    std::process::exit(1);
                }
            };

            let mut prepared_delete_symbols = match conn.prepare(
                "MATCH (f:File {path: $path})-[:CONTAINS*1..]->(s:Symbol) \
                 DETACH DELETE s"
            ) {
                Ok(stmt) => stmt,
                Err(err) => {
                    eprintln!("Error: Failed to prepare symbol cleanup query: {}", err);
                    std::process::exit(1);
                }
            };

            let mut prepared_symbol_create = match conn.prepare(
                "MERGE (s:Symbol {id: $id}) \
                 ON CREATE SET s.name = $name, s.kind = $kind, s.start_line = $start_line, s.start_col = $start_col, s.end_line = $end_line, s.signature = $signature \
                 ON MATCH SET s.name = $name, s.kind = $kind, s.start_line = $start_line, s.start_col = $start_col, s.end_line = $end_line, s.signature = $signature"
            ) {
                Ok(stmt) => stmt,
                Err(err) => {
                    eprintln!("Error: Failed to prepare symbol create query: {}", err);
                    std::process::exit(1);
                }
            };

            let mut prepared_containment_file = match conn.prepare(
                "MATCH (f:File {path: $from_id}), (s:Symbol {id: $to_id}) CREATE (f)-[:CONTAINS]->(s)"
            ) {
                Ok(stmt) => stmt,
                Err(err) => {
                    eprintln!("Error: Failed to prepare File containment query: {}", err);
                    std::process::exit(1);
                }
            };

            let mut prepared_containment_symbol = match conn.prepare(
                "MATCH (p:Symbol {id: $from_id}), (c:Symbol {id: $to_id}) CREATE (p)-[:CONTAINS]->(c)"
            ) {
                Ok(stmt) => stmt,
                Err(err) => {
                    eprintln!("Error: Failed to prepare Symbol containment query: {}", err);
                    std::process::exit(1);
                }
            };

            println!("--------------------------------------------------");
            println!("🔍 Traversing workspace & populating graph...");
            println!("--------------------------------------------------");

            let mut file_count = 0;
            let mut skip_count = 0;
            let mut byte_count = 0u64;

            let walker = WalkBuilder::new(&path).build();

            for result in walker {
                match result {
                    Ok(entry) => {
                        let file_path = entry.path();
                        if file_path.is_file() {
                            // Skip the database storage file and its companion WAL/temp files
                            let db_name = db_path.file_name().and_then(|n| n.to_str()).unwrap_or("synapse.lbug");
                            let file_name = file_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                            if file_name == db_name || file_name.starts_with(&format!("{}.", db_name)) {
                                continue;
                            }

                            let relative_path = file_path
                                .strip_prefix(&path)
                                .unwrap_or(file_path)
                                .to_path_buf();

                            let relative_path_str = relative_path.to_string_lossy().to_string();

                            if let Ok(metadata) = entry.metadata() {
                                let size = metadata.len();
                                let lang = detect_language(file_path);

                                match compute_sha256(file_path) {
                                    Ok(hash) => {
                                        // A. Check if file hash exists and matches
                                        let check_params: Vec<(&str, Value)> = vec![
                                            ("path", Value::String(relative_path_str.clone())),
                                        ];

                                        let mut matches = false;
                                        if let Ok(mut query_result) = conn.execute(&mut prepared_check_hash, check_params) {
                                            if let Some(row) = query_result.next() {
                                                if let Some(Value::String(stored_hash)) = row.first() {
                                                    if stored_hash == &hash {
                                                        matches = true;
                                                    }
                                                }
                                            }
                                        }

                                        if matches {
                                            skip_count += 1;
                                            if verbose {
                                                println!("Skipped (unchanged): {}", relative_path_str);
                                            }
                                            continue;
                                        }

                                        // B. Upsert File metadata in DB
                                        let params: Vec<(&str, Value)> = vec![
                                            ("path", Value::String(relative_path_str.clone())),
                                            ("language", Value::String(lang.clone())),
                                            ("file_size", Value::Int64(size as i64)),
                                            ("hash", Value::String(hash.clone())),
                                        ];

                                        match conn.execute(&mut prepared_file_upsert, params) {
                                            Ok(_) => {
                                                file_count += 1;
                                                byte_count += size;

                                                // C. Clean up old symbols and contains edges for this file
                                                let cleanup_params: Vec<(&str, Value)> = vec![
                                                    ("path", Value::String(relative_path_str.clone())),
                                                ];
                                                if let Err(err) = conn.execute(&mut prepared_delete_symbols, cleanup_params) {
                                                    eprintln!("Warning: Cleanup failed for '{}': {}", relative_path_str, err);
                                                }

                                                // D. Parse file AST content
                                                let mut parse_success = false;
                                                let ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
                                                let is_supported = matches!(ext.as_str(), "rs" | "js" | "jsx" | "ts" | "tsx");

                                                if is_supported {
                                                    if let Ok(mut file_handle) = std::fs::File::open(file_path) {
                                                        let mut content = String::new();
                                                        if file_handle.read_to_string(&mut content).is_ok() {
                                                            let (nodes, edges) = parser::ASTParser::parse_file(&relative_path, &content);

                                                            // E. Upsert Symbol nodes
                                                            for node in nodes {
                                                                let node_params: Vec<(&str, Value)> = vec![
                                                                    ("id", Value::String(node.id)),
                                                                    ("name", Value::String(node.name)),
                                                                    ("kind", Value::String(node.kind)),
                                                                    ("start_line", Value::Int64(node.start_line as i64)),
                                                                    ("start_col", Value::Int64(node.start_col as i64)),
                                                                    ("end_line", Value::Int64(node.end_line as i64)),
                                                                    ("signature", Value::String(node.signature)),
                                                                ];
                                                                let _ = conn.execute(&mut prepared_symbol_create, node_params);
                                                            }

                                                            // F. Insert CONTAINS relationships
                                                            for edge in edges {
                                                                let edge_params: Vec<(&str, Value)> = vec![
                                                                    ("from_id", Value::String(edge.from_id.clone())),
                                                                    ("to_id", Value::String(edge.to_id)),
                                                                ];
                                                                if edge.from_id == relative_path_str {
                                                                    let _ = conn.execute(&mut prepared_containment_file, edge_params);
                                                                } else {
                                                                    let _ = conn.execute(&mut prepared_containment_symbol, edge_params);
                                                                }
                                                            }
                                                            parse_success = true;
                                                        }
                                                    }
                                                }

                                                if verbose {
                                                    println!(
                                                        "Indexed: {} [{}] | Size: {} B | SHA-256: {} (parsed: {})",
                                                        relative_path_str,
                                                        lang,
                                                        size,
                                                        &hash[..8],
                                                        parse_success
                                                    );
                                                }
                                            }
                                            Err(err) => {
                                                eprintln!("Error: Failed to index file '{}' in database: {}", relative_path_str, err);
                                            }
                                        }
                                    }
                                    Err(err) => {
                                        if verbose {
                                            eprintln!("Warning: Failed to read '{}': {}", file_path.display(), err);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(err) => {
                        eprintln!("Workspace Traversal Error: {}", err);
                    }
                }
            }

            println!("--------------------------------------------------");
            println!("✅ Workspace traversal complete!");
            println!("Total Files Indexed/Updated    : {}", file_count);
            println!("Total Files Skipped (Unchanged): {}", skip_count);
            println!(
                "Aggregate Data Size Managed    : {:.2} MB",
                (byte_count as f64) / 1024.0 / 1024.0
            );
            println!("==================================================");
        }
    }
}
