//! Domain-error → rmcp::ErrorData mapping.

use rmcp::model::ErrorData;
use serde_json::{json, Value};
use thiserror::Error;

use crate::mcp::repo_registry::RegistryError;
use crate::mcp::tools::McpToolError;

/// Returned by query handlers when symbol/file resolution yields zero or
/// multiple candidates. Lives here for now; a future group moves it to
/// `crate::query` and refactors the handlers to return `Result<_, CandidateError>`
/// directly. `anyhow::Error::downcast_ref` requires this type to be
/// `std::error::Error + Send + Sync + 'static` — satisfied by the `thiserror`
/// derive plus the `String` / `Vec<Value>` payload (both are `Send + Sync`).
#[derive(Debug, Error)]
pub enum CandidateError {
    #[error("no symbol matches '{0}'")]
    NoMatch(String),
    #[error("'{0}' matches multiple candidates")]
    Ambiguous(String, Vec<Value>),
}

/// Convert an internal error into the MCP wire-format error data.
///
/// The mapping follows `openspec/changes/add-mcp-server/specs/mcp-server/spec.md`
/// `Structured Error Responses`. Agents match on `code` (MCP error code) plus
/// the structured `data` payload, not on free-text `message`.
///
/// Callers convert their domain errors via `anyhow::Error::new(...)` or the
/// `?` operator (which auto-lifts any `E: std::error::Error + Send + Sync + 'static`
/// into `anyhow::Error` via the `From` impl) before invoking this function.
pub fn map_domain_error(err: anyhow::Error) -> ErrorData {
    if let Some(candidate) = err.downcast_ref::<CandidateError>() {
        return match candidate {
            CandidateError::NoMatch(name) => ErrorData::invalid_params(
                format!("no symbol matches '{name}'"),
                None,
            ),
            CandidateError::Ambiguous(name, candidates) => ErrorData::invalid_params(
                format!("'{name}' matches multiple candidates"),
                Some(json!({ "candidates": candidates })),
            ),
        };
    }

    if let Some(reg_err) = err.downcast_ref::<RegistryError>() {
        return match reg_err {
            RegistryError::NoHome => ErrorData::invalid_params(
                "HOME is unset; cannot locate ~/.synapse/repos.json",
                None,
            ),
            RegistryError::DuplicatePath { existing_name, .. } => ErrorData::invalid_params(
                format!("repo already registered as '{existing_name}'"),
                None,
            ),
            RegistryError::DuplicateName { existing_path, .. } => ErrorData::invalid_params(
                format!("repo name already used by '{existing_path}'"),
                None,
            ),
            RegistryError::Io(_) | RegistryError::Json(_) => {
                ErrorData::internal_error(format!("registry error: {reg_err}"), None)
            }
        };
    }

    if let Some(tool_err) = err.downcast_ref::<McpToolError>() {
        return match tool_err {
            McpToolError::InvalidParams(msg) => ErrorData::invalid_params(
                format!("invalid parameters: {msg}"),
                None,
            ),
            McpToolError::NotFound(msg) => {
                ErrorData::invalid_params(format!("not found: {msg}"), None)
            }
            McpToolError::Internal(msg) => {
                ErrorData::internal_error(format!("internal error: {msg}"), None)
            }
        };
    }

    // TODO: replace this string-match heuristic with a typed
    // `EmbedderError::MissingEmbeddings` once `src/embedder.rs` exposes one.
    // The spec only mandates a recoverable-error message, not a typed path.
    let msg = err.to_string();
    if msg.contains("embedding") && msg.contains("not") {
        return ErrorData::invalid_params(
            "run 'synapse embed' to populate embeddings",
            None,
        );
    }

    // Unmapped error → internal. Don't leak raw stack traces or panic messages.
    ErrorData::internal_error(format!("internal error: {msg}"), None)
}

#[cfg(test)]
#[allow(clippy::expect_used)] // test fixtures assert on Option/Result values
mod tests {
    use super::*;
    use rmcp::model::ErrorCode;
    use serde_json::json;

    #[test]
    fn candidate_no_match_maps_to_invalid_params() {
        let err = anyhow::Error::new(CandidateError::NoMatch("foo".to_string()));
        let mapped = map_domain_error(err);
        assert_eq!(mapped.code, ErrorCode::INVALID_PARAMS);
        assert_eq!(mapped.message, "no symbol matches 'foo'");
        assert!(mapped.data.is_none());
    }

    #[test]
    fn candidate_ambiguous_maps_to_invalid_params_with_candidates_in_data() {
        let candidates = vec![
            json!({"name": "parse", "file": "src/parser.rs", "line": 10}),
            json!({"name": "parse", "file": "src/ast.rs", "line": 42}),
        ];
        let err = anyhow::Error::new(CandidateError::Ambiguous(
            "parse".to_string(),
            candidates.clone(),
        ));
        let mapped = map_domain_error(err);
        assert_eq!(mapped.code, ErrorCode::INVALID_PARAMS);
        assert_eq!(mapped.message, "'parse' matches multiple candidates");
        let data = mapped.data.expect("data should be set for ambiguous");
        let extracted = data.get("candidates").expect("candidates key in data");
        assert_eq!(extracted, &json!(candidates));
    }

    #[test]
    fn tool_invalid_params_maps_with_prefix() {
        let err = anyhow::Error::new(McpToolError::InvalidParams(
            "missing field 'name'".to_string(),
        ));
        let mapped = map_domain_error(err);
        assert_eq!(mapped.code, ErrorCode::INVALID_PARAMS);
        assert_eq!(mapped.message, "invalid parameters: missing field 'name'");
    }

    #[test]
    fn tool_internal_maps_to_internal_error_code() {
        let err = anyhow::Error::new(McpToolError::Internal("database corruption".to_string()));
        let mapped = map_domain_error(err);
        assert_eq!(mapped.code, ErrorCode::INTERNAL_ERROR);
        assert_eq!(mapped.message, "internal error: database corruption");
    }
}
