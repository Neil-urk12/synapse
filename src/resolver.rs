use crate::types::ast::{RawCall, RawImport};
use crate::types::db::{FileRecord, SymbolRecord};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

fn normalize_path(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            Component::Normal(c) => {
                normalized.push(c);
            }
            Component::RootDir | Component::Prefix(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

pub fn resolve_imports(files: &[FileRecord]) -> Vec<(String, String)> {
    let mut edges = HashSet::new();

    let file_paths: HashMap<String, &FileRecord> =
        files.iter().map(|f| (f.path.clone(), f)).collect();

    for file in files {
        if let Ok(raw_imports) = serde_json::from_str::<Vec<RawImport>>(&file.raw_imports) {
            let base_path = Path::new(&file.path);
            let base_dir = base_path.parent().unwrap_or(Path::new(""));
            let ext = base_path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let is_rust = ext == "rs";

            for imp in raw_imports {
                let mut resolved = false;

                // JS/TS Relative Path Resolution
                if imp.path.starts_with('.') {
                    let target_raw = base_dir.join(&imp.path);
                    let target_normalized = normalize_path(&target_raw);

                    for ext in &["ts", "tsx", "js", "jsx"] {
                        let path_with_ext = target_normalized.with_extension(ext);
                        let path_str = path_with_ext.to_string_lossy().to_string();
                        if file_paths.contains_key(&path_str) {
                            edges.insert((file.path.clone(), path_str));
                            resolved = true;
                            break;
                        }
                    }

                    if !resolved {
                        for ext in &["ts", "js"] {
                            let path_with_index = target_normalized.join(format!("index.{}", ext));
                            let path_str = path_with_index.to_string_lossy().to_string();
                            if file_paths.contains_key(&path_str) {
                                edges.insert((file.path.clone(), path_str));
                                break;
                            }
                        }
                    }
                }
                // Rust Module Resolution
                else if is_rust {
                    let segments: Vec<&str> = imp.path.split("::").collect();
                    if !segments.is_empty() {
                        let mut start_idx = 0;
                        let mut resolved_base_dir = PathBuf::from("src");

                        if segments[0] == "crate" {
                            start_idx = 1;
                        } else if segments[0] == "self" {
                            start_idx = 1;
                            resolved_base_dir = base_dir.to_path_buf();
                        } else {
                            let mut current_dir = base_dir;
                            while start_idx < segments.len() && segments[start_idx] == "super" {
                                current_dir = current_dir.parent().unwrap_or(Path::new(""));
                                start_idx += 1;
                                resolved_base_dir = current_dir.to_path_buf();
                            }
                        }

                        let mut last_resolved: Option<PathBuf> = None;
                        let mut current_base = resolved_base_dir;

                        for seg in segments.iter().skip(start_idx) {

                            let candidate_rs = current_base.join(format!("{}.rs", seg));
                            let candidate_mod = current_base.join(seg).join("mod.rs");

                            let norm_rs = normalize_path(&candidate_rs);
                            let norm_mod = normalize_path(&candidate_mod);

                            let found = if file_paths.contains_key(
                                &norm_rs.to_string_lossy().to_string(),
                            ) {
                                Some(norm_rs)
                            } else if file_paths.contains_key(
                                &norm_mod.to_string_lossy().to_string(),
                            ) {
                                Some(norm_mod)
                            } else {
                                None
                            };

                            match found {
                                Some(resolved_path) => {
                                    last_resolved = Some(resolved_path.clone());
                                    current_base = current_base.join(seg);
                                }
                                None => break,
                            }
                        }

                        if let Some(resolved) = last_resolved {
                            edges.insert((
                                file.path.clone(),
                                resolved.to_string_lossy().to_string(),
                            ));
                        }
                    }
                }
            }
        }
    }
    edges.into_iter().collect()
}

pub fn resolve_calls(
    symbols: &[SymbolRecord],
    import_edges: &[(String, String)],
) -> Vec<(String, String, usize)> {
    let mut resolved_edges = HashSet::new();

    let mut file_symbols: HashMap<String, Vec<&SymbolRecord>> = HashMap::new();
    let mut symbol_map: HashMap<String, &SymbolRecord> = HashMap::new();
    let mut name_map: HashMap<String, Vec<&SymbolRecord>> = HashMap::new();

    for sym in symbols {
        symbol_map.insert(sym.id.clone(), sym);
        name_map.entry(sym.name.clone()).or_default().push(sym);

        if let Some(file_path) = sym.id.split("::").next() {
            file_symbols
                .entry(file_path.to_string())
                .or_default()
                .push(sym);
        }
    }

    let mut file_imports: HashMap<String, Vec<String>> = HashMap::new();
    for (from_file, to_file) in import_edges {
        file_imports
            .entry(from_file.clone())
            .or_default()
            .push(to_file.clone());
    }

    for sym in symbols {
        if let Ok(raw_calls) = serde_json::from_str::<Vec<RawCall>>(&sym.raw_calls) {
            let file_path = sym.id.split("::").next().unwrap_or("");

            for call in raw_calls {
                let mut resolved = false;

                // Step 1: Surrounding Context (Method calls on classes/impl blocks)
                if call.is_method {
                    let segments: Vec<&str> = sym.id.split("::").collect();
                    if segments.len() >= 3 {
                        let parent_id = segments[..segments.len() - 1].join("::");
                        let method_id = format!("{}::{}", parent_id, call.name);
                        if symbol_map.contains_key(&method_id) {
                            resolved_edges.insert((sym.id.clone(), method_id, call.line));
                            resolved = true;
                        }
                    }
                }

                // Step 2: File Scope
                if !resolved {
                    if let Some(local_syms) = file_symbols.get(file_path) {
                        for local in local_syms {
                            if local.name == call.name {
                                resolved_edges.insert((
                                    sym.id.clone(),
                                    local.id.clone(),
                                    call.line,
                                ));
                                resolved = true;
                                break;
                            }
                        }
                    }
                }

                // Step 3: Import Scope
                if !resolved {
                    if let Some(imports) = file_imports.get(file_path) {
                        for imported_file in imports {
                            if let Some(imported_syms) = file_symbols.get(imported_file) {
                                for target in imported_syms {
                                    if target.name == call.name {
                                        resolved_edges.insert((
                                            sym.id.clone(),
                                            target.id.clone(),
                                            call.line,
                                        ));
                                        resolved = true;
                                        break;
                                    }
                                }
                            }
                            if resolved {
                                break;
                            }
                        }
                    }
                }

                // Step 4: Global Fallback
                if !resolved {
                    if let Some(matches) = name_map.get(&call.name) {
                        let mut candidates: Vec<&SymbolRecord> = matches
                            .iter()
                            .filter(|s| s.kind == "Function" || s.kind == "Method")
                            .cloned()
                            .collect();

                        if candidates.is_empty() {
                            candidates = matches.clone();
                        }

                        for candidate in candidates {
                            resolved_edges.insert((
                                sym.id.clone(),
                                candidate.id.clone(),
                                call.line,
                            ));
                        }
                    }
                }
            }
        }
    }

    resolved_edges.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file_record(path: &str, raw_imports: &str) -> FileRecord {
        FileRecord {
            path: path.to_string(),
            raw_imports: raw_imports.to_string(),
        }
    }

    fn symbol_record(id: &str, name: &str, kind: &str, raw_calls: &str) -> SymbolRecord {
        SymbolRecord {
            id: id.to_string(),
            name: name.to_string(),
            kind: kind.to_string(),
            raw_calls: raw_calls.to_string(),
        }
    }

    mod resolve_imports_tests {
        use super::*;

        #[test]
        fn rust_crate_import() {
            let files = vec![
                file_record("src/main.rs", r#"[{"path":"crate::parser","line":1}]"#),
                file_record("src/parser.rs", "[]"),
            ];
            let edges = resolve_imports(&files);
            assert!(edges.contains(&("src/main.rs".to_string(), "src/parser.rs".to_string())));
            assert_eq!(edges.len(), 1);
        }

        #[test]
        fn rust_self_import() {
            let files = vec![
                file_record("src/main.rs", "[]"),
                file_record("src/parser.rs", r#"[{"path":"self::inner","line":1}]"#),
                file_record("src/inner.rs", "[]"),
            ];
            let edges = resolve_imports(&files);
            assert!(edges.contains(&("src/parser.rs".to_string(), "src/inner.rs".to_string())));
        }

        #[test]
        fn rust_super_import() {
            let files = vec![
                file_record("src/mod.rs", "[]"),
                file_record("src/nested/child.rs", r#"[{"path":"super::mod","line":1}]"#),
            ];
            let edges = resolve_imports(&files);
            assert!(edges.contains(&("src/nested/child.rs".to_string(), "src/mod.rs".to_string())));
        }

        #[test]
        fn js_relative_import() {
            let files = vec![
                file_record("src/app.ts", r#"[{"path":"./helper","line":1}]"#),
                file_record("src/helper.ts", "[]"),
            ];
            let edges = resolve_imports(&files);
            assert!(edges.contains(&("src/app.ts".to_string(), "src/helper.ts".to_string())));
        }

        #[test]
        fn js_relative_index_import() {
            let files = vec![
                file_record("src/app.ts", r#"[{"path":"./utils","line":1}]"#),
                file_record("src/utils/index.ts", "[]"),
            ];
            let edges = resolve_imports(&files);
            assert!(edges.contains(&("src/app.ts".to_string(), "src/utils/index.ts".to_string())));
        }

        #[test]
        fn jsx_resolution() {
            let files = vec![
                file_record("src/App.tsx", r#"[{"path":"./Button","line":1}]"#),
                file_record("src/Button.tsx", "[]"),
            ];
            let edges = resolve_imports(&files);
            assert!(edges.contains(&("src/App.tsx".to_string(), "src/Button.tsx".to_string())));
        }

        #[test]
        fn no_match_returns_empty() {
            let files = vec![file_record(
                "src/main.rs",
                r#"[{"path":"crate::nonexistent","line":1}]"#,
            )];
            let edges = resolve_imports(&files);
            assert!(edges.is_empty());
        }

        #[test]
        fn invalid_json_skipped() {
            let files = vec![file_record("src/main.rs", "not json")];
            let edges = resolve_imports(&files);
            assert!(edges.is_empty());
        }

        #[test]
        fn rust_multi_segment_resolves_to_last_file() {
            let files = vec![
                file_record(
                    "src/main.rs",
                    r#"[{"path":"crate::foo::bar::Baz","line":1}]"#,
                ),
                file_record("src/foo.rs", "[]"),
                file_record("src/foo/bar.rs", "[]"),
            ];
            let edges = resolve_imports(&files);
            assert!(
                edges.contains(&("src/main.rs".to_string(), "src/foo/bar.rs".to_string())),
                "Should resolve to deepest module. Got: {:?}",
                edges
            );
            assert_eq!(edges.len(), 1);
        }

        #[test]
        fn rust_multi_segment_with_mod_rs() {
            let files = vec![
                file_record(
                    "src/main.rs",
                    r#"[{"path":"crate::a::b::Item","line":1}]"#,
                ),
                file_record("src/a.rs", "[]"),
                file_record("src/a/b/mod.rs", "[]"),
            ];
            let edges = resolve_imports(&files);
            assert!(
                edges.contains(&("src/main.rs".to_string(), "src/a/b/mod.rs".to_string())),
                "mod.rs path. Got: {:?}",
                edges
            );
        }

        #[test]
        fn rust_super_then_deep() {
            let files = vec![
                file_record(
                    "src/nested/child.rs",
                    r#"[{"path":"super::other::util::Util","line":1}]"#,
                ),
                file_record("src/other.rs", "[]"),
                file_record("src/other/util.rs", "[]"),
            ];
            let edges = resolve_imports(&files);
            assert!(
                edges.contains(&("src/nested/child.rs".to_string(), "src/other/util.rs".to_string())),
                "super then deep. Got: {:?}",
                edges
            );
        }

        #[test]
        fn rust_single_segment_regression() {
            let files = vec![
                file_record("src/main.rs", r#"[{"path":"crate::parser","line":1}]"#),
                file_record("src/parser.rs", "[]"),
            ];
            let edges = resolve_imports(&files);
            assert!(edges.contains(&("src/main.rs".to_string(), "src/parser.rs".to_string())));
        }
    }

    mod resolve_calls_tests {
        use super::*;

        #[test]
        fn file_scope_resolution() {
            let symbols = vec![
                symbol_record(
                    "src/main.rs::main",
                    "main",
                    "Function",
                    r#"[{"name":"helper","line":5,"is_method":false}]"#,
                ),
                symbol_record("src/main.rs::helper", "helper", "Function", "[]"),
            ];
            let edges = resolve_calls(&symbols, &[]);
            assert!(edges.contains(&(
                "src/main.rs::main".to_string(),
                "src/main.rs::helper".to_string(),
                5usize
            )));
        }

        #[test]
        fn import_scope_resolution() {
            let symbols = vec![
                symbol_record(
                    "src/main.rs::main",
                    "main",
                    "Function",
                    r#"[{"name":"parse","line":10,"is_method":false}]"#,
                ),
                symbol_record("src/parser.rs::parse", "parse", "Function", "[]"),
            ];
            let import_edges = vec![("src/main.rs".to_string(), "src/parser.rs".to_string())];
            let edges = resolve_calls(&symbols, &import_edges);
            assert!(edges.contains(&(
                "src/main.rs::main".to_string(),
                "src/parser.rs::parse".to_string(),
                10usize
            )));
        }

        #[test]
        fn method_call_step1() {
            let symbols = vec![
                symbol_record(
                    "src/lib.rs::MyStruct::do_work",
                    "do_work",
                    "Method",
                    r#"[{"name":"helper","line":3,"is_method":true}]"#,
                ),
                symbol_record("src/lib.rs::MyStruct::helper", "helper", "Method", "[]"),
            ];
            let edges = resolve_calls(&symbols, &[]);
            assert!(edges.contains(&(
                "src/lib.rs::MyStruct::do_work".to_string(),
                "src/lib.rs::MyStruct::helper".to_string(),
                3usize
            )));
        }

        #[test]
        fn global_fallback() {
            let symbols = vec![
                symbol_record(
                    "src/main.rs::main",
                    "main",
                    "Function",
                    r#"[{"name":"parse_file","line":7,"is_method":false}]"#,
                ),
                symbol_record("src/parser.rs::parse_file", "parse_file", "Function", "[]"),
            ];
            // No import edge — should still resolve via global fallback
            let edges = resolve_calls(&symbols, &[]);
            assert!(edges.iter().any(|(from, to, _line)| {
                from == "src/main.rs::main" && to == "src/parser.rs::parse_file"
            }));
        }

        #[test]
        fn unknown_call_returns_empty() {
            let symbols = vec![symbol_record(
                "src/main.rs::main",
                "main",
                "Function",
                r#":[{"name":"does_not_exist","line":1,"is_method":false}]"#,
            )];
            let edges = resolve_calls(&symbols, &[]);
            assert!(edges.is_empty());
        }

        #[test]
        fn invalid_calls_json_skipped() {
            let symbols = vec![symbol_record(
                "src/main.rs::main",
                "main",
                "Function",
                "bad json",
            )];
            let edges = resolve_calls(&symbols, &[]);
            assert!(edges.is_empty());
        }
    }
}
