# Synapse

Synapse is a high-performance code intelligence graph database and indexer built with **Rust** and powered by **[LadybugDB](https://github.com/ladybugdb/ladybug)** (embedded graph + vector + relational database).

## Features

- **Fast & Incremental**: Parallel workspace traversal respecting `.gitignore`, using SHA-256 hashing to skip unchanged files.
- **Multi-Language AST Parsing**: Extracts declarations from 11 language families using tree-sitter: Rust, JavaScript/JSX, TypeScript/TSX, Go, Python, C/C++, Java, Kotlin, Ruby, PHP, Swift.
- **Graph Linking**: Resolves module imports (`IMPORTS`) and call sites (`CALLS`) into a global code graph.
- **Semantic Search**: Computes vector embeddings (BGE-small-en-v1.5, 384-dim) over code chunks for natural-language similarity search.
- **Code Chunking**: Segments code by symbol boundaries or sliding windows to prepare for embeddings.
- **File Watcher**: Watches for file changes and automatically re-indexes with configurable debounce.
- **Query CLI**: Subcommands for callers, callees, dependencies, context, blast-radius (`impact`), and raw Cypher queries.
- **PageRank Scoring**: Ranks symbols by transitive importance over the CALLS graph; surfaces hubs and lets `impact` prioritize affected code.
- **Interactive REPL**: Shell with history, shortcuts, and raw Cypher query support.

## Quick Start

### Build

```bash
cargo build
```

### Index Codebase

```bash
# Index current directory (default: synapse.lbug)
cargo run -- index .

# Custom path, custom DB, verbose output
cargo run -- index /path/to/project -d myproj.lbug -v
```

### Query Symbols

```bash
# Find all callers of a symbol
cargo run -- callers <symbol>

# Find all callees of a symbol
cargo run -- callees <symbol>

# Find file dependencies (imports and imported-by)
cargo run -- deps <file>

# Get rich code context for a symbol or file
cargo run -- context --symbol <symbol>
cargo run -- context --file <file> --format json

# Blast radius: show every symbol that transitively depends on a target,
# sorted by PageRank. Answers "what breaks if I change this?"
cargo run -- impact <symbol>
cargo run -- blast-radius src/foo.rs::bar --format json --top 20
```

All query commands support `--exact` for exact matching (default is fuzzy) and `--format` for output in `table`, `markdown`, or `json`.

### Semantic Search

```bash
# Compute embeddings for all indexed chunks
cargo run -- embed

# Search code by natural language
cargo run -- similar "error handling" --limit 10 --threshold 0.7
```

### File Watcher

```bash
# Watch directory and auto-reindex on changes
cargo run -- watch . --debounce 3 -v
```

### Raw Cypher Queries

```bash
# Execute a raw Cypher query against the graph
cargo run -- query "MATCH (f:File) RETURN f.path LIMIT 10"
```

### Interactive REPL

```bash
cargo run -- repl
```

Shortcuts: `.help`, `.callers <symbol>`, `.callees <symbol>`, `.deps <file>`, `.context <symbol>`, `.exit`/`.quit`. Any non-shortcut input is executed as a raw Cypher query.

### Test

```bash
cargo test
```
