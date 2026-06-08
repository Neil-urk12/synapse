# Synapse

Synapse is a high-performance data integration layer built with **Rust** and powered by **[LadybugDB](https://github.com/ladybugdb/ladybug)**, the embedded graph+vector+relational database.

## Features

- **Fast**: Native Rust performance with zero-copy data paths and LadybugDB's in-process execution model.
- **Graph + Vector + SQL**: Query, traverse, and embed in a single unified engine.
- **Embedded**: Runs in-process — no separate database server to deploy or manage.
- **Type-safe**: Strongly-typed Rust APIs with compile-time query validation where possible.
- **Workspace Traversal**: Efficiently walks target directories, respecting `.gitignore` rules and skipping hidden files/folders (such as `.git/`) by default.

## Tech Stack

- **Language**: Rust (edition 2021, MSRV 1.75)
- **Database**: [LadybugDB](https://github.com/ladybugdb/ladybug) — an embedded, multi-model (graph / vector / relational) database built for analytical and transactional workloads.

## Quick Start

### Prerequisites

- Rust toolchain (1.75+)
- LadybugDB native library installed on your system

### Build

```bash
cargo build --release
```

### Run

```bash
cargo run
```

### Test

```bash
cargo test
```

## Project Status

🚧 **Early development** — APIs and storage schemas are subject to change.

## License

TBD
