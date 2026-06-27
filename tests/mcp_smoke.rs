//! End-to-end smoke test for the MCP server.
//!
//! Spawns the `synapse mcp` binary as a child process, sends a minimal
//! JSON-RPC handshake over its stdin, and asserts that:
//!
//! 1. `initialize` returns a `serverInfo` with `name = "synapse"`
//! 2. `tools/list` returns the 8 expected `synapse_*` tool names
//! 3. `resources/list` returns `synapse://repos`
//!
//! This is intentionally minimal — the full 14-tool surface is covered
//! by per-tool unit tests in `src/mcp/tool_impls.rs` and `src/mcp/server.rs`.
//! This test exists to catch integration regressions in the wire protocol
//! (transport, request routing, response shape).

#![allow(clippy::expect_used)]

use std::io::Write;
use std::process::{Command, Stdio};

/// Locate the synapse binary built by `cargo test` (target/debug/synapse).
/// `CARGO_BIN_EXE_synapse` is set by cargo's integration-test harness when
/// the binary is built with `[[bin]] name = "synapse"` (which it is).
fn synapse_binary() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_synapse"))
}

/// Send a single JSON-RPC request (no trailing newline — we add it).
/// Reads the response synchronously and returns the parsed JSON value.
///
/// **Notifications (requests without an `id` field) get no response** per the
/// JSON-RPC 2.0 spec — `read_line` after a notification would block forever.
/// We detect them via the absence of `id` and return `None` instead.
fn send_request(
    stdin: &mut std::process::ChildStdin,
    stdout: &mut std::process::ChildStdout,
    request: serde_json::Value,
) -> Option<serde_json::Value> {
    let is_notification = request.get("id").is_none();
    let payload = format!("{}\n", request);
    stdin
        .write_all(payload.as_bytes())
        .expect("write to mcp stdin");
    stdin.flush().expect("flush mcp stdin");

    if is_notification {
        return None;
    }

    let mut buf = String::new();
    use std::io::BufRead;
    let mut reader = std::io::BufReader::new(stdout);
    reader
        .read_line(&mut buf)
        .expect("read line from mcp stdout");
    Some(serde_json::from_str(buf.trim()).expect("parse JSON-RPC response"))
}

#[test]
fn mcp_server_handshake_reports_9_synapse_tools() {
    let mut child = Command::new(synapse_binary())
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn synapse mcp");

    let mut stdin = child.stdin.take().expect("mcp stdin");
    let mut stdout = child.stdout.take().expect("mcp stdout");

    // 1. initialize handshake
    let init_resp = send_request(
        &mut stdin,
        &mut stdout,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "synapse-smoke-test",
                    "version": "0.0.0"
                }
            }
        }),
    )
    .expect("initialize should return a response");
    let server_info = init_resp
        .get("result")
        .and_then(|r| r.get("serverInfo"))
        .expect("initialize response should include serverInfo");
    assert_eq!(server_info["name"], "synapse");

    // 2. initialized notification — no response per JSON-RPC spec.
    assert!(
        send_request(
            &mut stdin,
            &mut stdout,
            serde_json::json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized"
            }),
        )
        .is_none(),
        "notifications/initialized is a notification — must return None, not a response"
    );

    // 3. tools/list
    let tools_resp = send_request(
        &mut stdin,
        &mut stdout,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }),
    )
    .expect("tools/list should return a response");
    let tools = tools_resp
        .get("result")
        .and_then(|r| r.get("tools"))
        .and_then(|t| t.as_array())
        .expect("tools/list response should include result.tools array");
    let names: Vec<&str> = tools
        .iter()
        .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
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
    for name in &expected {
        assert!(
            names.contains(name),
            "tools/list should include '{name}'. Got: {names:?}"
        );
    }
    assert_eq!(
        names.len(),
        expected.len(),
        "tools/list returned {} tools, expected {}",
        names.len(),
        expected.len()
    );

    // 4. resources/list (just the fixed `synapse://repos` for v1)
    let resources_resp = send_request(
        &mut stdin,
        &mut stdout,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "resources/list",
            "params": {}
        }),
    )
    .expect("resources/list should return a response");
    let resources = resources_resp
        .get("result")
        .and_then(|r| r.get("resources"))
        .and_then(|r| r.as_array())
        .expect("resources/list response should include result.resources array");
    let uris: Vec<&str> = resources
        .iter()
        .filter_map(|r| r.get("uri").and_then(|u| u.as_str()))
        .collect();
    assert!(
        uris.contains(&"synapse://repos"),
        "resources/list should include 'synapse://repos'. Got: {uris:?}"
    );

    // 5. clean shutdown — close stdin so the server sees EOF and exits.
    drop(stdin);
    drop(stdout);
    let status = child.wait().expect("wait for mcp child");
    // Exit code may be 0 (clean) or non-zero (broken pipe); both are acceptable
    // since we forcibly closed the pipes. The functional assertions above are
    // what matter.
    let _ = status;
}
