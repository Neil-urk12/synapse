use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

/// SHA-256 hash for incremental indexing detection.
pub fn compute_sha256(path: &Path) -> io::Result<String> {
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

/// Extract source code lines from a file's content string.
/// Lines are 1-indexed. Returns empty string for invalid ranges.
pub fn slice_source_code(content: &str, start_line: usize, end_line: usize) -> String {
    if start_line == 0 || end_line == 0 || start_line > end_line {
        return String::new();
    }
    let lines: Vec<&str> = content.lines().collect();
    let start_idx = start_line - 1;
    let end_idx = std::cmp::min(end_line, lines.len());
    if start_idx >= lines.len() {
        return String::new();
    }
    lines[start_idx..end_idx].join("\n")
}

/// Detect language from file extension.
pub fn detect_language(path: &Path) -> String {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("rs") => "Rust".to_string(),
        Some("js") | Some("jsx") => "JavaScript".to_string(),
        Some("ts") | Some("tsx") => "TypeScript".to_string(),
        Some("py") => "Python".to_string(),
        Some("go") => "Go".to_string(),
        Some("cpp") | Some("cc") | Some("cxx") | Some("hpp") => "C++".to_string(),
        Some("c") | Some("h") => "C".to_string(),
        Some("java") => "Java".to_string(),
        Some("kt") | Some("kts") => "Kotlin".to_string(),
        Some("html") => "HTML".to_string(),
        Some("css") => "CSS".to_string(),
        Some("md") => "Markdown".to_string(),
        Some("json") => "JSON".to_string(),
        _ => "Unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slice_source_code() {
        let content = "line 1\nline 2\nline 3\nline 4\nline 5";
        let sliced = slice_source_code(content, 2, 4);
        assert_eq!(sliced, "line 2\nline 3\nline 4");
    }

    #[test]
    fn test_slice_source_code_invalid_range() {
        let content = "line 1\nline 2\nline 3";
        assert_eq!(slice_source_code(content, 0, 2), "");
        assert_eq!(slice_source_code(content, 2, 0), "");
        assert_eq!(slice_source_code(content, 3, 2), "");
    }

    #[test]
    fn test_slice_source_code_out_of_bounds() {
        let content = "line 1\nline 2";
        assert_eq!(slice_source_code(content, 5, 6), "");
        let sliced = slice_source_code(content, 1, 10);
        assert_eq!(sliced, "line 1\nline 2");
    }
}
