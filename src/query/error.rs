use serde::Serialize;
use thiserror::Error;

/// Structured candidate item returned when a query target matches multiple symbols or files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CandidateItem {
    pub name: String,
    pub file: String,
    pub line: usize,
}

/// Typed domain errors produced by the code intelligence query engine.
#[derive(Debug, Error)]
pub enum QueryError {
    #[error("No match found for '{0}'")]
    NotFound(String),

    #[error("No match found for '{target}'. Did you mean: {}", suggestions.join(", "))]
    NotFoundWithSuggestions {
        target: String,
        suggestions: Vec<String>,
    },

    #[error("'{target}' matches multiple candidates")]
    Ambiguous {
        target: String,
        candidates: Vec<CandidateItem>,
    },

    #[error("Database error: {0}")]
    Database(String),

    #[error("Embedder error: {0}")]
    Embedder(String),

    #[error("{0}")]
    InvalidInput(String),

    #[error("{0}")]
    NotEmbedded(String),
}

impl From<lbug::Error> for QueryError {
    fn from(e: lbug::Error) -> Self {
        QueryError::Database(e.to_string())
    }
}
