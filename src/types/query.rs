#[derive(Debug, Clone, serde::Serialize)]
pub struct SymbolInfo {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub start_line: usize,
    pub end_line: usize,
    pub signature: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FileInfo {
    pub path: String,
    pub language: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CallerInfo {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub signature: String,
    pub call_site_line: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CalleeInfo {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub signature: String,
    pub call_site_line: usize,
}

impl From<CalleeInfo> for CallerInfo {
    fn from(c: CalleeInfo) -> Self {
        Self {
            id: c.id,
            name: c.name,
            kind: c.kind,
            signature: c.signature,
            call_site_line: c.call_site_line,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ContainedSymbolInfo {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub signature: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ContextPayload {
    pub symbol: Option<SymbolInfo>,
    pub file: Option<FileInfo>,
    pub source_code: String,
    pub callers: Vec<CallerInfo>,
    pub callees: Vec<CalleeInfo>,
    pub imports: Vec<String>,
    pub imported_by: Vec<String>,
    pub contained_symbols: Vec<ContainedSymbolInfo>,
}
