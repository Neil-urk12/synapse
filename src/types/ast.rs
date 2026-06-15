#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct RawImport {
    pub path: String,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct RawCall {
    pub name: String,
    pub line: usize,
    pub is_method: bool,
}

#[derive(Debug, Clone)]
pub struct FileAnalysis {
    pub nodes: Vec<NodeData>,
    pub edges: Vec<EdgeData>,
    pub imports: Vec<RawImport>,
    pub calls: Vec<RawCall>,
}

impl FileAnalysis {
    pub fn empty() -> Self {
        FileAnalysis {
            nodes: Vec::new(),
            edges: Vec::new(),
            imports: Vec::new(),
            calls: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeData {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeData {
    pub from_id: String,
    pub to_id: String,
    pub edge_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct CodeChunk {
    pub id: String,
    pub text: String,
    pub symbol_id: Option<String>,
}
