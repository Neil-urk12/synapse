//! Internal McpTool trait and tool registry (rmcp-agnostic).

use std::collections::HashMap;

use serde_json::Value;

/// Internal abstraction over a single MCP tool. Tool handlers accept JSON args
/// and return JSON results; the rmcp wrapper in `mod.rs` adapts these to/from
/// the wire protocol. Keeping this trait rmcp-agnostic isolates the rest of the
/// MCP code from upstream API churn.
#[async_trait::async_trait]
pub trait McpTool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    /// Input JSON shape is tool-specific; output JSON shape is documented per
    /// tool's spec section in `openspec/changes/add-mcp-server/specs/mcp-server/spec.md`.
    async fn invoke(&self, args: Value) -> Result<Value, McpToolError>;
}

/// Tool-specific error type. Maps to `rmcp::McpError` at the wire boundary in
/// `errors.rs` (Group 5). Kept as a flat enum here so each tool can return
/// structured, agent-readable errors without dragging rmcp into module scope.
#[derive(Debug, thiserror::Error)]
pub enum McpToolError {
    #[error("invalid parameters: {0}")]
    InvalidParams(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("internal error: {0}")]
    Internal(String),
}

pub struct ToolRegistry {
    tools: HashMap<&'static str, Box<dyn McpTool>>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a tool. Panics if a tool with the same name is already
    /// registered — this is a programmer error caught at startup, not a
    /// runtime failure mode.
    pub fn register(&mut self, tool: Box<dyn McpTool>) {
        let name = tool.name();
        assert!(
            !self.tools.contains_key(name),
            "MCP tool '{name}' registered twice"
        );
        self.tools.insert(name, tool);
    }

    pub fn get(&self, name: &str) -> Option<&dyn McpTool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    pub fn list(&self) -> Vec<(&'static str, &'static str)> {
        let mut v: Vec<_> = self
            .tools
            .values()
            .map(|t| (t.name(), t.description()))
            .collect();
        v.sort_by_key(|(n, _)| *n);
        v
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)] // test fixtures assert registered / looked-up values
mod tests {
    use super::*;
    use serde_json::json;

    struct StubTool;
    #[async_trait::async_trait]
    impl McpTool for StubTool {
        fn name(&self) -> &'static str {
            "stub"
        }
        fn description(&self) -> &'static str {
            "A stub tool for testing"
        }
        async fn invoke(&self, _args: Value) -> Result<Value, McpToolError> {
            Ok(json!({"ok": true}))
        }
    }

    #[test]
    fn register_and_get_roundtrips() {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(StubTool));
        let got = reg.get("stub").expect("stub tool should be registered");
        assert_eq!(got.name(), "stub");
        assert_eq!(got.description(), "A stub tool for testing");
    }

    #[test]
    #[should_panic(expected = "registered twice")]
    fn register_duplicate_panics() {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(StubTool));
        reg.register(Box::new(StubTool));
    }

    #[test]
    fn list_returns_sorted() {
        let mut reg = ToolRegistry::new();
        // Second stub under a different name to keep `register` happy.
        struct StubTool2;
        #[async_trait::async_trait]
        impl McpTool for StubTool2 {
            fn name(&self) -> &'static str {
                "alpha"
            }
            fn description(&self) -> &'static str {
                "first"
            }
            async fn invoke(&self, _args: Value) -> Result<Value, McpToolError> {
                Ok(json!({}))
            }
        }
        reg.register(Box::new(StubTool));
        reg.register(Box::new(StubTool2));
        let names: Vec<_> = reg.list().into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, vec!["alpha", "stub"]);
    }
}
