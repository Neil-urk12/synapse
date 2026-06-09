# Synapse

Synapse is a high-performance code intelligence graph database and indexer built with **Rust** and powered by **[LadybugDB](https://github.com/ladybugdb/ladybug)** (embedded graph + vector + relational database).

## Features

- **Fast & Incremental**: Traverses workspaces respecting `.gitignore`, using SHA-256 hashing to skip unchanged files.
- **AST Parsing**: Multi-language support (Rust, JS/JSX, TS/TSX) using tree-sitter to extract declarations (Functions, Structs, Classes, Methods, Interfaces).
- **Graph Linking**: Dynamically resolves module imports (`IMPORTS`) and call sites (`CALLS`) to assemble a global code graph.
- **Code Chunking**: Segments code by symbol boundaries or sliding windows to prepare for semantic vector embeddings.
- **Interactive REPL**: Shell editor with history and shortcuts for querying callers, callees, dependencies, and code context.
- **Query CLI**: Direct subcommands to query callers, callees, and file dependencies with support for table, markdown, and JSON outputs.

## Quick Start

### Build

```bash
cargo build
```

### Index Codebase

```bash
# Index current directory into default synapse.lbug database
cargo run -- index .

# Index custom path with custom DB and verbose logging
cargo run -- index /path/to/project --db myproj.lbug -v
```

### Direct Queries

```bash
# Find callers of a symbol
cargo run -- callers run_query

# Find callees of a symbol
cargo run -- callees handle_repl_command

# Find file dependencies (imports and imported-by)
cargo run -- deps src/main.rs
```

### Interactive Query Shell (REPL)

```bash
cargo run -- repl
```
Inside the REPL, use shortcuts like `.help`, `.callers`, `.callees`, `.deps`, `.context`, or run raw database queries directly.

### Test

```bash
cargo test
```
