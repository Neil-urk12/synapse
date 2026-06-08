use std::fs::File;
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use ignore::WalkBuilder;
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

fn compute_sha256(path: &Path) -> io::Result<String> {
    let mut file = BufReader::new(File::open(path)?);
    let mut hasher = Sha256::new();
    io::copy(&mut file, &mut hasher)?;
    Ok(hex::encode(hasher.finalize()))
}

fn detect_language(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()? {
        "rs" => Some("Rust"),
        "ts" | "tsx" => Some("TypeScript"),
        "js" | "jsx" => Some("JavaScript"),
        "py" => Some("Python"),
        "go" => Some("Go"),
        "java" => Some("Java"),
        "c" | "h" => Some("C"),
        "cpp" | "hpp" | "cc" | "cxx" => Some("C++"),
        "rb" => Some("Ruby"),
        "cs" => Some("CSharp"),
        _ => None,
    }
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Index { path, db, verbose } => {
            println!("==================================================");
            println!("Synapse Indexer Initializing");
            println!("==================================================");
            println!("Workspace Target : {}", path.display());
            println!("Database Target  : {}", db.display());
            println!("--------------------------------------------------");

            if !path.exists() {
                eprintln!("Error: Target workspace path '{}' does not exist.", path.display());
                std::process::exit(1);
            }

            let mut file_count = 0u64;
            let mut byte_count = 0u64;

            let walker = WalkBuilder::new(&path).build();

            for result in walker {
                match result {
                    Ok(entry) => {
                        let file_path = entry.path();
                        if !file_path.is_file() {
                            continue;
                        }

                        let relative_path = file_path.strip_prefix(&path).unwrap_or(file_path);
                        let metadata = match entry.metadata() {
                            Ok(m) => m,
                            Err(err) => {
                                if verbose {
                                    eprintln!("Warning: metadata for '{}': {}", file_path.display(), err);
                                }
                                continue;
                            }
                        };

                        let size = metadata.len();
                        let language = detect_language(file_path);

                        match compute_sha256(file_path) {
                            Ok(hash) => {
                                file_count += 1;
                                byte_count += size;

                                if verbose {
                                    println!(
                                        "Indexed: {} | {} | {} B | SHA-256: {}",
                                        relative_path.display(),
                                        language.unwrap_or("Unknown"),
                                        size,
                                        &hash[..8],
                                    );
                                }
                            }
                            Err(err) => {
                                if verbose {
                                    eprintln!("Warning: Failed to read '{}': {}", file_path.display(), err);
                                }
                            }
                        }
                    }
                    Err(err) => {
                        eprintln!("Workspace traversal error: {}", err);
                    }
                }
            }

            println!("--------------------------------------------------");
            println!("Workspace traversal complete.");
            println!("Total files tracked : {}", file_count);
            println!("Aggregate data size : {:.2} MB", byte_count as f64 / 1_048_576.0);
            println!("==================================================");
        }
    }
}
