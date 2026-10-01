//! Domain-error → rmcp::ErrorData mapping.

use rmcp::model::ErrorData;
use serde_json::json;

use crate::mcp::tools::McpToolError;

/// Convert tool errors into MCP error codes, messages, and structured data.
pub fn map_domain_error(err: McpToolError) -> ErrorData {
    match err {
        McpToolError::Ambiguous(name, candidates) => ErrorData::invalid_params(
            format!("'{name}' matches multiple candidates"),
            Some(json!({ "candidates": candidates })),
        ),
        McpToolError::InvalidParams(msg) => {
            ErrorData::invalid_params(format!("invalid parameters: {msg}"), None)
        }
        McpToolError::NotFound(msg) => ErrorData::invalid_params(format!("not found: {msg}"), None),
        McpToolError::Internal(msg) => {
            ErrorData::internal_error(format!("internal error: {msg}"), None)
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)] // test fixtures assert on Option/Result values
mod tests {
    use super::*;
    use rmcp::model::ErrorCode;
    use serde_json::json;

    #[test]
    fn tool_not_found_maps_with_prefix() {
        let mapped = map_domain_error(McpToolError::NotFound("foo".to_string()));
        assert_eq!(mapped.code, ErrorCode::INVALID_PARAMS);
        assert_eq!(mapped.message, "not found: foo");
        assert!(mapped.data.is_none());
    }

    #[test]
    fn tool_invalid_params_maps_with_prefix() {
        let err = McpToolError::InvalidParams("missing field 'name'".to_string());
        let mapped = map_domain_error(err);
        assert_eq!(mapped.code, ErrorCode::INVALID_PARAMS);
        assert_eq!(mapped.message, "invalid parameters: missing field 'name'");
    }

    #[test]
    fn tool_internal_maps_to_internal_error_code() {
        let err = McpToolError::Internal("database corruption".to_string());
        let mapped = map_domain_error(err);
        assert_eq!(mapped.code, ErrorCode::INTERNAL_ERROR);
        assert_eq!(mapped.message, "internal error: database corruption");
    }

    /// `McpToolError::Ambiguous` maps to `ErrorCode::INVALID_PARAMS` with
    /// `data.candidates` populated.
    #[test]
    fn tool_ambiguous_re_emits_candidates() {
        let candidates = vec![
            json!({"name": "parse", "file": "src/parser.rs", "line": 10}),
            json!({"name": "parse", "file": "src/ast.rs", "line": 42}),
        ];
        let err = McpToolError::Ambiguous("parse".to_string(), candidates.clone());
        let mapped = map_domain_error(err);
        assert_eq!(mapped.code, ErrorCode::INVALID_PARAMS);
        assert_eq!(mapped.message, "'parse' matches multiple candidates");
        let data = mapped.data.expect("data should be set for ambiguous");
        let extracted = data.get("candidates").expect("candidates key in data");
        assert_eq!(extracted, &json!(candidates));
    }
}
