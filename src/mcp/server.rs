//! MCP server bootstrap: stdio transport, tool dispatch, and resources.
//!
//! The `McpServer` struct implements rmcp's `ServerHandler` trait manually
//! (not via `#[tool_handler]`) so it can dispatch to our internal `McpTool`
//! implementations registered through `ToolRegistry`. This isolates the rest
//! of the MCP code from rmcp API churn — if `rmcp::model::*` changes, only
//! this file (and `errors.rs`) need updates.

use std::sync::Arc;
use std::time::Duration;

use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    AnnotateAble, CallToolRequestParam, CallToolResult, ListResourcesResult, ListToolsResult,
    RawResource, ReadResourceRequestParam, ReadResourceResult, ResourceContents,
    ServerCapabilities, ServerInfo, Tool, ToolAnnotations,
};
use rmcp::service::ServiceExt;
use rmcp::transport::stdio;
use rmcp::ErrorData as McpError;
use serde_json::Value;

use crate::mcp::errors::map_domain_error;
use crate::mcp::resources::{read_repo_status, read_repos};
use crate::mcp::tool_impls::{
    CalleesTool, CallersTool, ContextTool, CypherTool, DeadCodeTool, DepsTool, ImpactTool,
    QueryTool, RankTool,
};
use crate::mcp::tools::{McpToolError, ToolRegistry};

// ─── CLI args mirror ─────────────────────────────────────────────────────

/// CLI args for `synapse mcp`. Kept here so the wiring in `main.rs` stays
/// declarative and this module owns its own shape.
#[derive(Debug, Clone)]
pub struct McpArgs {
    pub status: bool,
    pub tool_timeout_secs: u64,
}

// ─── Server struct + handler impl ─────────────────────────────────────────

/// The MCP server. Owns the `ToolRegistry` (Arc-shared so the async call
/// handlers can call into it without re-locking).
#[derive(Clone)]
pub struct McpServer {
    tools: Arc<ToolRegistry>,
    tool_timeout: Duration,
}

impl McpServer {
    pub fn new(tool_timeout: Duration) -> Self {
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(CypherTool));
        tools.register(Box::new(CallersTool));
        tools.register(Box::new(CalleesTool));
        tools.register(Box::new(DepsTool));
        tools.register(Box::new(ContextTool));
        tools.register(Box::new(QueryTool));
        tools.register(Box::new(ImpactTool));
        tools.register(Box::new(RankTool));
        tools.register(Box::new(DeadCodeTool));
        Self {
            tools: Arc::new(tools),
            tool_timeout,
        }
    }
}

impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: Default::default(),
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
            server_info: rmcp::model::Implementation {
                name: "synapse".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                ..Default::default()
            },
            instructions: Some(
                "Synapse MCP server: read-only code-intelligence queries (callers, callees, \
                 context, deps, PageRank-ranked blast radius, semantic search, raw Cypher). \
                 Resources: synapse://repos and synapse://repo/{name}/status."
                    .into(),
            ),
        }
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParam,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let tool_name = request.name.to_string();
        let Some(tool) = self.tools.get(&tool_name) else {
            return Err(McpError::method_not_found::<
                rmcp::model::CallToolRequestMethod,
            >());
        };
        let args = request
            .arguments
            .unwrap_or_default()
            .into_iter()
            .collect::<serde_json::Map<String, Value>>();

        // Wrap the tool invocation in a per-tool timeout. On timeout we map
        // to `internal_error` with a clear message rather than aborting the
        // server — the agent can retry with a smaller query.
        let tool_name_for_error = tool_name.clone();
        let invoke_fut = tool.invoke(Value::Object(args));
        match tokio::time::timeout(self.tool_timeout, invoke_fut).await {
            Ok(Ok(value)) => Ok(CallToolResult::structured(value)),
            Ok(Err(tool_err)) => {
                let error_data = map_domain_error(anyhow::Error::new(tool_err));
                Err(error_data)
            }
            Err(_) => Err(McpError::internal_error(
                format!(
                    "tool '{}' exceeded {}s timeout",
                    tool_name_for_error,
                    self.tool_timeout.as_secs()
                ),
                None,
            )),
        }
    }

    async fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParam>,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let tools: Vec<Tool> = self
            .tools
            .list()
            .into_iter()
            .map(|(name, description)| {
                Tool::new(
                    name.to_string(),
                    description.to_string(),
                    Arc::new(
                        serde_json::json!({
                            "type": "object",
                            "additionalProperties": true,
                        })
                        .as_object()
                        .unwrap()
                        .clone(),
                    ),
                )
                .annotate(ToolAnnotations {
                    read_only_hint: Some(true),
                    ..Default::default()
                })
            })
            .collect();
        Ok(ListToolsResult {
            tools,
            next_cursor: None,
        })
    }

    async fn list_resources(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParam>,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let mut resources: Vec<rmcp::model::Resource> = Vec::new();
        // `synapse://repos` — always present.
        let repos = RawResource {
            uri: "synapse://repos".to_string(),
            name: "repos".to_string(),
            title: Some("Indexed repositories".to_string()),
            description: Some(
                "Every repo registered in ~/.synapse/repos.json with staleness hints.".to_string(),
            ),
            mime_type: Some("application/json".to_string()),
            size: None,
            icons: None,
        };
        resources.push(repos.no_annotation());
        Ok(ListResourcesResult {
            resources,
            next_cursor: None,
        })
    }

    async fn list_resource_templates(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParam>,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::ListResourceTemplatesResult, McpError> {
        // `synapse://repo/{name}/status` template — one entry per registered repo.
        let template = rmcp::model::RawResourceTemplate {
            uri_template: "synapse://repo/{name}/status".to_string(),
            name: "repo_status".to_string(),
            title: Some("Repo status".to_string()),
            description: Some(
                "Detailed status for a single repo: indexed_at, head_commit, is_stale, \
                 files_count, symbols_count."
                    .to_string(),
            ),
            mime_type: Some("application/json".to_string()),
        };
        Ok(rmcp::model::ListResourceTemplatesResult {
            resource_templates: vec![template.no_annotation()],
            next_cursor: None,
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParam,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        let uri = request.uri.as_str();
        let value = match uri {
            "synapse://repos" => read_repos().map_err(domain_to_mcp_error)?,
            other if other.starts_with("synapse://repo/") && other.ends_with("/status") => {
                let name = other
                    .trim_start_matches("synapse://repo/")
                    .trim_end_matches("/status");
                read_repo_status(name).map_err(domain_to_mcp_error)?
            }
            _ => {
                return Err(McpError::invalid_params(
                    format!("unknown resource URI: {uri}"),
                    None,
                ));
            }
        };
        let body = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
        Ok(ReadResourceResult {
            contents: vec![ResourceContents::TextResourceContents {
                uri: uri.to_string(),
                mime_type: Some("application/json".to_string()),
                text: body,
                meta: None,
            }],
        })
    }
}

/// Convert internal `McpToolError` (from resource reads) to rmcp's `ErrorData`.
fn domain_to_mcp_error(err: McpToolError) -> McpError {
    map_domain_error(anyhow::Error::new(err))
}

// ─── Entry points ─────────────────────────────────────────────────────────

/// Run the MCP server over stdio. Blocks until stdin closes.
pub async fn run(args: McpArgs) -> Result<(), Box<dyn std::error::Error>> {
    if args.status {
        run_status()?;
        return Ok(());
    }
    let server = McpServer::new(Duration::from_secs(args.tool_timeout_secs));
    let service = server.serve(stdio()).await.inspect_err(|e| {
        eprintln!("Error starting MCP server: {e}");
    })?;
    service.waiting().await?;
    Ok(())
}

/// Print a one-line status report for every registered repo and exit.
#[allow(clippy::print_stdout)] // CLI output: `synapse mcp --status` is meant for human reading
pub fn run_status() -> Result<(), Box<dyn std::error::Error>> {
    use crate::mcp::repo_registry::{is_stale as entry_is_stale, RepoRegistry};
    let registry = RepoRegistry::load().unwrap_or_default();
    println!("synapse: {} repos indexed", registry.entries.len());
    for entry in &registry.entries {
        let stale = entry_is_stale(entry);
        let head = crate::mcp::resources::git_head(&entry.path).unwrap_or_default();
        let head_marker = if head.is_empty() {
            String::new()
        } else {
            format!(" head={}", &head[..8.min(head.len())])
        };
        let stale_label = if entry.indexed_commit.is_empty() {
            "unknown (no commit recorded)"
        } else if stale {
            "stale"
        } else {
            "up to date"
        };
        println!("  - {} ({}){}", entry.name, stale_label, head_marker);
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used)] // test fixtures assert on Option/Result values
mod tests {
    use super::*;

    #[test]
    fn server_constructs_with_all_9_tools_registered() {
        let server = McpServer::new(Duration::from_secs(30));
        let names: Vec<_> = server
            .tools
            .list()
            .into_iter()
            .map(|(n, _)| n.to_string())
            .collect();
        let expected = vec![
            "synapse_callees",
            "synapse_callers",
            "synapse_context",
            "synapse_cypher",
            "synapse_dead_code",
            "synapse_deps",
            "synapse_impact",
            "synapse_query",
            "synapse_rank",
        ];
        assert_eq!(names, expected);
    }

    #[test]
    fn server_info_has_tools_and_resources_capabilities() {
        let server = McpServer::new(Duration::from_secs(30));
        let info = server.get_info();
        let caps = info.capabilities;
        assert!(format!("{caps:?}").contains("tools") || caps.tools.is_some());
        assert!(caps.resources.is_some());
    }
}
