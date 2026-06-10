pub mod chunker;
pub mod embed;
pub mod embedder;
pub mod file_utils;
pub mod index;
pub mod linker;
pub mod parser;
pub mod query_cli;
pub mod resolver;
pub mod schema;
pub mod similar;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

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
    /// Execute a raw Cypher query against the code graph
    Query {
        /// The Cypher query string to execute
        query: String,

        /// Path to the LadybugDB database storage file
        #[arg(short, long, default_value = "synapse.lbug")]
        db: PathBuf,
    },
    /// Retrieve and format code intelligence context for a symbol or file
    Context {
        /// Target symbol name
        #[arg(short, long)]
        symbol: Option<String>,

        /// Target file path
        #[arg(short, long)]
        file: Option<String>,

        /// Enable fuzzy/case-insensitive substring search
        #[arg(short = 'z', long)]
        fuzzy: bool,

        /// Output format: markdown or json
        #[arg(long, default_value = "markdown")]
        format: String,

        /// Path to the LadybugDB database storage file
        #[arg(short, long, default_value = "synapse.lbug")]
        db: PathBuf,
    },
    /// Find all callers of a target symbol
    Callers {
        /// Target symbol name
        symbol: String,

        /// Match exactly instead of fuzzy substring match
        #[arg(short, long)]
        exact: bool,

        /// Output format: table, markdown, json
        #[arg(long, default_value = "table")]
        format: String,

        /// Path to the LadybugDB database storage file
        #[arg(short, long, default_value = "synapse.lbug")]
        db: PathBuf,
    },
    /// Find all callees of a target symbol
    Callees {
        /// Target symbol name
        symbol: String,

        /// Match exactly instead of fuzzy substring match
        #[arg(short, long)]
        exact: bool,

        /// Output format: table, markdown, json
        #[arg(long, default_value = "table")]
        format: String,

        /// Path to the LadybugDB database storage file
        #[arg(short, long, default_value = "synapse.lbug")]
        db: PathBuf,
    },
    /// List all dependencies (imports and imported-by) for a file
    #[command(alias = "deps")]
    Dependencies {
        /// Target file path
        file: String,

        /// Match exactly instead of fuzzy substring match
        #[arg(short, long)]
        exact: bool,

        /// Output format: table, markdown, json
        #[arg(long, default_value = "table")]
        format: String,

        /// Path to the LadybugDB database storage file
        #[arg(short, long, default_value = "synapse.lbug")]
        db: PathBuf,
    },
    /// Start an interactive query shell (REPL)
    Repl {
        /// Path to the LadybugDB database storage file
        #[arg(short, long, default_value = "synapse.lbug")]
        db: PathBuf,
    },
    /// Compute semantic embeddings for all code chunks
    Embed {
        /// Path to the LadybugDB database storage file
        #[arg(short, long, default_value = "synapse.lbug")]
        db: PathBuf,

        /// Batch size for embedding computation
        #[arg(short, long, default_value = "256")]
        batch_size: usize,

        /// Enable verbose output with progress
        #[arg(short, long)]
        verbose: bool,
    },
    /// Search code chunks by semantic similarity
    Similar {
        /// Natural language query to search for
        query: String,

        /// Path to the LadybugDB database storage file
        #[arg(short, long, default_value = "synapse.lbug")]
        db: PathBuf,

        /// Maximum number of results
        #[arg(short, long, default_value = "5")]
        limit: usize,

        /// Minimum cosine similarity threshold (0.0-1.0)
        #[arg(short, long, default_value = "0.5")]
        threshold: f32,

        /// Output format: markdown or json
        #[arg(long, default_value = "markdown")]
        format: String,
    },
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
            index::run_index(path, db_path, verbose);
        }
        Commands::Query { query, db } => {
            if let Err(err) = query_cli::handle_query(&query, &db) {
                eprintln!("Error: {}", err);
                std::process::exit(1);
            }
        }
        Commands::Context {
            symbol,
            file,
            fuzzy,
            format,
            db,
        } => {
            if let Err(err) =
                query_cli::handle_context(symbol.as_deref(), file.as_deref(), fuzzy, &format, &db)
            {
                eprintln!("Error: {}", err);
                std::process::exit(1);
            }
        }
        Commands::Callers {
            symbol,
            exact,
            format,
            db,
        } => {
            if let Err(err) = query_cli::handle_callers(&symbol, exact, &format, &db) {
                eprintln!("Error: {}", err);
                std::process::exit(1);
            }
        }
        Commands::Callees {
            symbol,
            exact,
            format,
            db,
        } => {
            if let Err(err) = query_cli::handle_callees(&symbol, exact, &format, &db) {
                eprintln!("Error: {}", err);
                std::process::exit(1);
            }
        }
        Commands::Dependencies {
            file,
            exact,
            format,
            db,
        } => {
            if let Err(err) = query_cli::handle_dependencies(&file, exact, &format, &db) {
                eprintln!("Error: {}", err);
                std::process::exit(1);
            }
        }
        Commands::Repl { db } => {
            if let Err(err) = query_cli::run_repl(&db) {
                eprintln!("Error: {}", err);
                std::process::exit(1);
            }
        }
        Commands::Embed {
            db,
            batch_size,
            verbose,
        } => {
            if let Err(err) = embed::handle_embed(&db, batch_size, verbose) {
                eprintln!("Error: {}", err);
                std::process::exit(1);
            }
        }
        Commands::Similar {
            query,
            db,
            limit,
            threshold,
            format,
        } => {
            if let Err(err) = similar::handle_similar(&query, &db, limit, threshold, &format) {
                eprintln!("Error: {}", err);
                std::process::exit(1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_parsing_callers() {
        let args = vec![
            "synapse",
            "callers",
            "test_symbol",
            "--exact",
            "--format",
            "json",
            "-d",
            "test_db.lbug",
        ];
        let parsed = Cli::try_parse_from(args).unwrap();
        match parsed.command {
            Commands::Callers {
                symbol,
                exact,
                format,
                db,
            } => {
                assert_eq!(symbol, "test_symbol");
                assert!(exact);
                assert_eq!(format, "json");
                assert_eq!(db, PathBuf::from("test_db.lbug"));
            }
            _ => panic!("Expected Callers variant"),
        }
    }

    #[test]
    fn test_cli_parsing_callees() {
        let args = vec![
            "synapse",
            "callees",
            "test_symbol",
            "--exact",
            "--format",
            "markdown",
            "-d",
            "test_db.lbug",
        ];
        let parsed = Cli::try_parse_from(args).unwrap();
        match parsed.command {
            Commands::Callees {
                symbol,
                exact,
                format,
                db,
            } => {
                assert_eq!(symbol, "test_symbol");
                assert!(exact);
                assert_eq!(format, "markdown");
                assert_eq!(db, PathBuf::from("test_db.lbug"));
            }
            _ => panic!("Expected Callees variant"),
        }
    }

    #[test]
    fn test_cli_parsing_dependencies() {
        let args = vec![
            "synapse",
            "dependencies",
            "test_file.rs",
            "--exact",
            "--format",
            "table",
            "-d",
            "test_db.lbug",
        ];
        let parsed = Cli::try_parse_from(args).unwrap();
        match parsed.command {
            Commands::Dependencies {
                file,
                exact,
                format,
                db,
            } => {
                assert_eq!(file, "test_file.rs");
                assert!(exact);
                assert_eq!(format, "table");
                assert_eq!(db, PathBuf::from("test_db.lbug"));
            }
            _ => panic!("Expected Dependencies variant"),
        }
    }

    #[test]
    fn test_cli_parsing_embed() {
        let args = vec!["synapse", "embed", "-d", "test.lbug", "-b", "128", "-v"];
        let parsed = Cli::try_parse_from(args).unwrap();
        match parsed.command {
            Commands::Embed {
                db,
                batch_size,
                verbose,
            } => {
                assert_eq!(db, PathBuf::from("test.lbug"));
                assert_eq!(batch_size, 128);
                assert!(verbose);
            }
            _ => panic!("Expected Embed variant"),
        }
    }

    #[test]
    fn test_cli_parsing_similar() {
        let args = vec![
            "synapse",
            "similar",
            "error handling",
            "-d",
            "test.lbug",
            "--limit",
            "10",
            "--threshold",
            "0.7",
            "--format",
            "json",
        ];
        let parsed = Cli::try_parse_from(args).unwrap();
        match parsed.command {
            Commands::Similar {
                query,
                db,
                limit,
                threshold,
                format,
            } => {
                assert_eq!(query, "error handling");
                assert_eq!(db, PathBuf::from("test.lbug"));
                assert_eq!(limit, 10);
                assert!((threshold - 0.7).abs() < 0.001);
                assert_eq!(format, "json");
            }
            _ => panic!("Expected Similar variant"),
        }
    }

    #[test]
    fn test_cli_parsing_repl() {
        let args = vec!["synapse", "repl", "-d", "test_db.lbug"];
        let parsed = Cli::try_parse_from(args).unwrap();
        match parsed.command {
            Commands::Repl { db } => {
                assert_eq!(db, PathBuf::from("test_db.lbug"));
            }
            _ => panic!("Expected Repl variant"),
        }
    }
}
