#[derive(Debug, Clone, serde::Deserialize)]
pub struct FileRecord {
    pub path: String,
    pub raw_imports: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct SymbolRecord {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub raw_calls: String,
}

pub struct ParsedPayload {
    pub relative_path: String,
    pub language: String,
    pub size: u64,
    pub hash: String,
    pub analysis: Option<crate::types::ast::FileAnalysis>,
    pub content: Option<String>,
}
