use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use crate::file_utils;
use crate::parser::MAX_PARSE_BYTES;
use crate::parser::{language_from_path, ASTParser};
use crate::types::db::ParsedPayload;

/// Build a `ParsedPayload` for one file. The shared seam used by both the
/// parallel index walker and the serial watch batch.
///
/// Returns:
/// - `Ok(None)` if the file's hash matches the cache (caller should skip).
/// - `Ok(Some(payload))` if the file should be written. `payload.analysis` is
///   `Some(...)` for parseable files, `None` for unparseable ones (which
///   still get a `File` node, but no `Symbol`s or `Chunk`s).
/// - `Err(_)` on I/O failure. The caller logs and continues.
///
/// This replaces the per-file logic that used to live inline inside
/// `index::run_index` and `watch::process_batch`, and the
/// `is_supported = "rs"|"js"|...` literal that diverged from the watch's
/// `should_watch` list.
pub fn process_file(
    abs_path: &Path,
    workspace_root: &Path,
    hash_cache: &HashMap<String, String>,
) -> Result<Option<ParsedPayload>, Box<dyn std::error::Error>> {
    let relative_path = abs_path
        .strip_prefix(workspace_root)
        .unwrap_or(abs_path)
        .to_string_lossy()
        .to_string();

    let hash = file_utils::compute_sha256(abs_path)?;
    if let Some(stored) = hash_cache.get(&relative_path) {
        if stored == &hash {
            return Ok(None);
        }
    }

    let language = file_utils::detect_language(abs_path);
    // Fail visibly on stat errors so a transient I/O issue doesn't get
    // masked as size=0 (which would bypass the MAX_PARSE_BYTES guard).
    // Callers (index.rs, watch.rs) log the warning and continue.
    let metadata = std::fs::metadata(abs_path).map_err(|err| {
        eprintln!("Warning: Cannot stat '{}': {}", relative_path, err);
        err
    })?;
    let size = metadata.len();

    if size > MAX_PARSE_BYTES {
        eprintln!(
            "Warning: skipping parse of '{}' ({} bytes exceeds {} byte limit)",
            relative_path, size, MAX_PARSE_BYTES
        );
        return Ok(Some(ParsedPayload {
            relative_path,
            language,
            size,
            hash,
            analysis: None,
            content: None,
        }));
    }

    let (analysis, content) = match (language_from_path(abs_path), std::fs::File::open(abs_path)) {
        (Some(_), Ok(mut file)) => {
            let mut text = String::new();
            match file.read_to_string(&mut text) {
                Ok(_) => (
                    Some(ASTParser::parse_file(Path::new(&relative_path), &text)),
                    Some(text),
                ),
                Err(_) => (None, None),
            }
        }
        _ => (None, None),
    };

    Ok(Some(ParsedPayload {
        relative_path,
        language,
        size,
        hash,
        analysis,
        content,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// Regression for the bug where `index.rs::is_supported` was a hardcoded
    /// list of the original 5 languages, so Go/Python/C++/etc. files were
    /// indexed as `File` nodes with empty `raw_imports` and no `Symbol`s.
    /// After the FileProcessor seam, parseability is decided by the Language
    /// registry, so a Go file is parsed end-to-end.
    #[test]
    fn test_process_file_parses_go_file() {
        let dir = tempdir().unwrap();
        let go_file = dir.path().join("main.go");
        fs::write(
            &go_file,
            "package main\n\nfunc Add(a, b int) int { return a + b }\n",
        )
        .unwrap();

        let cache = HashMap::new();
        let result = process_file(&go_file, dir.path(), &cache).unwrap();

        let payload = result.expect("Go file should not be skipped on first run");
        assert_eq!(payload.language, "Go");
        let analysis = payload
            .analysis
            .expect("Go file should be parsed (regression: is_supported bug)");
        assert!(
            !analysis.nodes.is_empty(),
            "Go file should produce at least one Symbol node"
        );
    }

    /// Unparseable files still get a `File` node (for tracking), but no
    /// `Symbol`s or `Chunk`s. Matches the index.rs behavior pre-refactor.
    #[test]
    fn test_process_file_creates_file_node_for_unparseable_file() {
        let dir = tempdir().unwrap();
        let txt_file = dir.path().join("notes.txt");
        fs::write(&txt_file, "just some prose").unwrap();

        let cache = HashMap::new();
        let result = process_file(&txt_file, dir.path(), &cache).unwrap();

        let payload = result.expect("Unparseable file should not be skipped");
        assert_eq!(payload.language, "Unknown");
        assert!(
            payload.analysis.is_none(),
            "Unparseable file should not produce an analysis"
        );
        assert!(
            payload.content.is_none(),
            "Unparseable file should not store its content"
        );
    }

    /// A file whose hash matches the cache is skipped. The caller doesn't
    /// need to write to the channel/DB.
    #[test]
    fn test_process_file_skips_unchanged_file() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("main.rs");
        fs::write(&file, "fn main() {}").unwrap();

        let hash = file_utils::compute_sha256(&file).unwrap();
        let mut cache = HashMap::new();
        // The relative path is what process_file keys on; compute it the
        // same way the function does.
        let relative = file
            .strip_prefix(dir.path())
            .unwrap_or(&file)
            .to_string_lossy()
            .to_string();
        cache.insert(relative, hash);

        let result = process_file(&file, dir.path(), &cache).unwrap();
        assert!(result.is_none(), "Unchanged file should be skipped");
    }

    /// A file whose hash differs from the cache is re-processed. Confirms
    /// the skip is driven by hash equality, not by presence in the cache.
    #[test]
    fn test_process_file_reprocesses_changed_file() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("main.rs");
        fs::write(&file, "fn main() {}").unwrap();

        let mut cache = HashMap::new();
        let relative = file
            .strip_prefix(dir.path())
            .unwrap_or(&file)
            .to_string_lossy()
            .to_string();
        cache.insert(relative, "stale-hash-from-prior-run".to_string());

        let result = process_file(&file, dir.path(), &cache).unwrap();
        let payload = result.expect("Changed file should not be skipped");
        assert!(payload.analysis.is_some());
        let result = process_file(&file, dir.path(), &cache).unwrap();
        let payload = result.expect("Changed file should not be skipped");
        assert!(payload.analysis.is_some());
    }

    /// Regression: `process_file` must call `ASTParser::parse_file` with the
    /// *relative* path, not the absolute one. The language parsers use that
    /// path to build `NodeData.id` and the top-level `EdgeData.from_id`
    /// (e.g. `parser/rust.rs`: `format!("{}::{}", ctx.file_path, name)` and
    /// `active_parent.as_deref().unwrap_or(ctx.file_path)`).
    ///
    /// `index::write_payload_to_db` then routes containment edges by
    /// comparing `edge.from_id == payload.relative_path` to decide between
    /// the file-level and symbol-level CONTAINS statements. If symbol IDs
    /// encode the absolute path, both branches fail their MATCH (no `File`
    /// is keyed by the absolute path, and no `Symbol` is keyed by it
    /// either), so no `CONTAINS` edges are created at all.
    #[test]
    fn test_process_file_uses_relative_path_in_symbol_ids() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("lib.rs");
        fs::write(&file, "pub fn alpha() {}\npub fn beta() {}\n").unwrap();

        let cache = HashMap::new();
        let payload = process_file(&file, dir.path(), &cache)
            .unwrap()
            .expect("first run should produce a payload");

        let analysis = payload
            .analysis
            .expect("Rust file must produce an analysis");
        assert!(
            !analysis.nodes.is_empty(),
            "expected at least two top-level symbols (alpha, beta)"
        );

        let expected_prefix = "lib.rs";
        for node in &analysis.nodes {
            assert!(
                node.id.starts_with(expected_prefix),
                "Symbol id '{}' must start with the relative path '{}'",
                node.id,
                expected_prefix,
            );
            assert!(
                !node.id.starts_with(dir.path().to_str().unwrap()),
                "Symbol id '{}' must NOT contain the absolute workspace path",
                node.id,
            );
        }

        for edge in &analysis.edges {
            assert_eq!(
                edge.from_id, expected_prefix,
                "Top-level containment edge from_id must equal the relative path, not the absolute one. Got '{}'",
                edge.from_id,
            );
        }
    }

    /// Files larger than `MAX_PARSE_BYTES` are skipped before being read or
    /// parsed. The file still becomes a `File` node (no `Symbol`s, no
    /// `Chunk`s, no stored content) so the indexer can list it; this
    /// mirrors the graceful-degradation pattern used for unparseable files.
    /// `process_file` writes the size-cap warning to stderr itself; we don't
    /// capture it here.
    #[test]
    fn test_process_file_skips_oversized_file() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("big.rs");
        // 6 MiB of filler — one byte over the 5 MiB cap.
        let oversize = vec![b'a'; 6 * 1024 * 1024];
        fs::write(&file, &oversize).unwrap();

        let cache = HashMap::new();
        let result = process_file(&file, dir.path(), &cache).unwrap();
        let payload = result.expect("Oversized file should still be indexed (no Symbols)");

        assert!(
            payload.analysis.is_none(),
            "Oversized file must not produce an analysis"
        );
        assert!(
            payload.content.is_none(),
            "Oversized file must not store its content"
        );
    }
}
