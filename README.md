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
- **`synapse impact <symbol>`**: Blast-radius analysis. Find every symbol transitively affected by changing the given function/method, sorted by PageRank. The "what breaks if I change this?" primitive.
- **`synapse rank [--top N] [--kind ...]`**: Top symbols by PageRank. Surface the structural hubs of your codebase over the CALLS graph.
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

### Model Context Protocol (MCP) Server

`synapse mcp` exposes 8 read-only code-intelligence tools and 2 discovery resources to any MCP-aware agent (Claude Code, Cursor, Windsurf, Codex, OpenCode, Antigravity, etc.) over **stdio**:

```bash
# Start the MCP server (blocks until stdin closes)
cargo run -- mcp

# Sanity check: list indexed repos and exit
cargo run -- mcp --status
```

**Tools** (namespaced with `synapse_` to avoid collisions with other MCP servers in the same agent):

| Tool | Purpose |
|---|---|
| `synapse_cypher` | Run a raw Cypher query against the code graph |
| `synapse_callers` | Find direct and transitive callers of a symbol |
| `synapse_callees` | Find what a symbol calls |
| `synapse_context` | Code intelligence for a symbol or file (signature, source slice, call graph, dependencies) |
| `synapse_deps` | Imports + imported-by for a file |
| `synapse_impact` | Blast radius (transitive reverse-CALLS), sorted by PageRank |
| `synapse_query` | Semantic search over indexed code chunks (requires `synapse embed`) |
| `synapse_rank` | Top symbols by PageRank |

**Resources**: `synapse://repos` (every indexed repo with staleness hints) and `synapse://repo/{name}/status` (detail per repo).

**Editor install:**

```jsonc
// Claude Code / Cursor / Windsurf — ~/.claude/mcp.json or .cursor/mcp.json
{
  "mcpServers": {
    "synapse": {
      "command": "synapse",
      "args": ["mcp"]
    }
  }
}
```

For Codex / OpenCode / Antigravity, the equivalent is `~/.codex/mcp.json`, `~/.config/opencode/mcp.json`, etc. — same shape. Restart the editor after editing the config.

The first call to any tool that reads from a DB will look for `./synapse.lbug` in the server's working directory. Run `synapse index .` from your project root before launching the MCP server.

### Test

```bash
cargo test
```
