use crate::types::query::{FileInfo, SymbolInfo};

pub fn resolve_symbol_candidates(
    target: &str,
    fuzzy: bool,
    pool: &[SymbolInfo],
) -> Vec<SymbolInfo> {
    pool.iter()
        .filter(|s| {
            if target.contains("::") {
                if fuzzy {
                    s.id.to_lowercase().contains(&target.to_lowercase())
                } else {
                    s.id == target
                }
            } else {
                if fuzzy {
                    s.name.to_lowercase().contains(&target.to_lowercase())
                } else {
                    s.name == target
                }
            }
        })
        .cloned()
        .collect()
}

pub fn resolve_file_candidates(target: &str, fuzzy: bool, pool: &[FileInfo]) -> Vec<FileInfo> {
    pool.iter()
        .filter(|f| {
            if fuzzy {
                f.path.to_lowercase().contains(&target.to_lowercase())
            } else {
                f.path == target
            }
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fuzzy_filtering() {
        let symbols = vec![
            SymbolInfo {
                id: "src/main.rs::main".to_string(),
                name: "main".to_string(),
                kind: "Function".to_string(),
                start_line: 1,
                end_line: 5,
                signature: "fn main()".to_string(),
            },
            SymbolInfo {
                id: "src/linker.rs::run_linker".to_string(),
                name: "run_linker".to_string(),
                kind: "Function".to_string(),
                start_line: 10,
                end_line: 20,
                signature: "fn run_linker()".to_string(),
            },
        ];

        let matches = resolve_symbol_candidates("run_linker", false, &symbols);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].id, "src/linker.rs::run_linker");

        let matches_fuzzy = resolve_symbol_candidates("LINK", true, &symbols);
        assert_eq!(matches_fuzzy.len(), 1);
        assert_eq!(matches_fuzzy[0].id, "src/linker.rs::run_linker");

        let matches_id = resolve_symbol_candidates("src/linker.rs::run_linker", false, &symbols);
        assert_eq!(matches_id.len(), 1);
        assert_eq!(matches_id[0].id, "src/linker.rs::run_linker");

        let matches_id_fuzzy = resolve_symbol_candidates("linker.rs::run", true, &symbols);
        assert_eq!(matches_id_fuzzy.len(), 1);
        assert_eq!(matches_id_fuzzy[0].id, "src/linker.rs::run_linker");
    }
}
