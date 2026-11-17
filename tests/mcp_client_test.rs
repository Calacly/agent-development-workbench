//! Interop tests for the MCP client against real servers.
//!
//! The protocol tests spawn the Python fixtures in `tests/fixtures/` with
//! `uv run --script`, so they need `uv` and network access on first run.
//! Run them with `cargo test --test mcp_client_test -- --ignored`.

use mcp_client::services::mcp_client::{McpClientService, McpTransport};
use std::path::PathBuf;

fn fixture_transport(script: &str) -> McpTransport {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(script);
    McpTransport::Stdio {
        command: "uv".to_string(),
        args: vec![
            "run".to_string(),
            "--quiet".to_string(),
            "--script".to_string(),
            path.to_string_lossy().into_owned(),
        ],
        env: None,
    }
}

/// Connect, check the negotiated protocol version, call `echo`, disconnect.
async fn assert_echo_roundtrip(script: &str, expect_v2: bool) {
    let service = McpClientService::new();

    let protocol_version = service
        .connect_server(1, script.to_string(), fixture_transport(script), None, false, None, None)
        .await
        .unwrap_or_else(|e| panic!("connect to {script} failed: {e}"))
        .expect("negotiated protocol version");

    if expect_v2 {
        assert_eq!(protocol_version, "2026-07-28", "{script} should negotiate MCP v2");
    } else {
        assert!(
            protocol_version.as_str() < "2026-07-28",
            "{script} should fall back to a legacy version, got {protocol_version}"
        );
    }

    let tools = service.list_tools(1).await.expect("list tools");
    assert!(tools.iter().any(|t| t.name == "echo"), "echo tool missing: {tools:#?}");

    let result = service
        .call_tool(1, "echo".to_string(), serde_json::json!({ "text": "hello" }))
        .await
        .expect("call echo");
    let text = serde_json::to_string(&result).unwrap();
    assert!(text.contains("hello"), "unexpected echo result: {text}");

    service.disconnect_server(1).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore] // Requires uv and network access
async fn test_connects_to_mcp_v2_server() {
    assert_echo_roundtrip("echo_server_v2.py", true).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore] // Requires uv and network access
async fn test_falls_back_to_legacy_server() {
    assert_echo_roundtrip("echo_server_legacy.py", false).await;
}

#[tokio::test]
async fn test_service_creation() {
    let service = McpClientService::new();
    assert_eq!(service.list_connected_servers().await.len(), 0);
    assert!(!service.is_connected(1).await);
}
