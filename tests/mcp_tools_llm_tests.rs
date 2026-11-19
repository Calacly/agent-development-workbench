//! API test for POST /mcp-servers/:id/test-tools: the LLM asks for a tool,
//! the backend runs it on a real MCP server (tests/fixtures/echo_server_v2.py)
//! and sends the result back, and the final text answer is returned.
//!
//! Needs `uv` and network access on first run:
//! cargo test --test mcp_tools_llm_tests -- --ignored

use axum::{http::StatusCode as AxumStatus, response::IntoResponse, routing::post, Json, Router};
use reqwest::StatusCode;
use serde_json::{json, Value};
use std::path::PathBuf;

mod common;
use common::setup_test_app;

/// Mock OpenAI-compatible LLM: asks for `echo` first, then answers with the
/// tool result it received, or with HTTP 429 when `rate_limit_second_call`.
async fn start_mock_llm(rate_limit_second_call: bool) -> String {
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move |Json(body): Json<Value>| async move {
            let messages = body["messages"].as_array().cloned().unwrap_or_default();
            // Strict like LiteLLM/vLLM: tool call arguments in the history must be strings
            let object_arguments = messages.iter().any(|m| {
                m["tool_calls"].as_array().is_some_and(|calls| calls.iter().any(|c| !c["function"]["arguments"].is_string()))
            });
            if object_arguments {
                return (
                    AxumStatus::BAD_REQUEST,
                    Json(json!({ "error": { "message": "function.arguments: Input should be a valid string" } })),
                )
                    .into_response();
            }
            let tool_reply = messages
                .iter()
                .find(|m| m["role"] == "tool")
                .and_then(|m| m["content"].as_str().map(str::to_string));

            if rate_limit_second_call && tool_reply.is_some() {
                return (
                    AxumStatus::TOO_MANY_REQUESTS,
                    Json(json!({ "error": { "message": "Rate limit reached for model `mock`", "code": "rate_limit_exceeded" } })),
                )
                    .into_response();
            }
            let message = match tool_reply {
                Some(text) => json!({ "role": "assistant", "content": format!("The tool said: {text}") }),
                None => json!({
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "echo", "arguments": "{\"text\":\"good morning\"}" }
                    }]
                }),
            };
            Json(json!({
                "id": "chatcmpl-mock",
                "object": "chat.completion",
                "created": 0,
                "model": body["model"],
                "choices": [{ "index": 0, "message": message, "finish_reason": "stop" }],
                "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
            }))
            .into_response()
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}/v1")
}

/// Create a provider for the mock LLM and a connected echo MCP server, call
/// test-tools, and return (status, body).
async fn run_test_tools(rate_limit_second_call: bool, max_tool_result_tokens: Option<i64>) -> (StatusCode, Value) {
    let app = setup_test_app().await;
    let token = app.register_and_login("toolllm", "toolllm@example.com", "testpass123").await;
    let llm = start_mock_llm(rate_limit_second_call).await;

    let provider: Value = app
        .client
        .post(format!("{}/llm-providers", app.api_url))
        .bearer_auth(&token)
        .json(&json!({ "name": "Mock", "base_url": llm, "api_key": "k", "model": "qwen3.8-27b-stable", "is_default": true }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    if let Some(tokens) = max_tool_result_tokens {
        let put = app
            .client
            .put(format!("{}/llm-providers/{}", app.api_url, provider["id"]))
            .bearer_auth(&token)
            .json(&json!({ "max_tool_result_tokens": tokens }))
            .send()
            .await
            .unwrap();
        assert_eq!(put.status(), StatusCode::OK);
    }

    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/echo_server_v2.py");
    let server: Value = app
        .client
        .post(format!("{}/mcp-servers", app.api_url))
        .bearer_auth(&token)
        .json(&json!({
            "name": "echo",
            "server_type": "stdio",
            "config": { "command": "uv", "args": ["run", "--quiet", "--script", fixture], "env": {} },
            "use_container": false
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let server_id = server["id"].as_i64().unwrap();

    let connect = app
        .client
        .post(format!("{}/mcp-servers/{server_id}/connect", app.api_url))
        .bearer_auth(&token)
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(connect.status(), StatusCode::OK, "{}", connect.text().await.unwrap());

    let response = app
        .client
        .post(format!("{}/mcp-servers/{server_id}/test-tools", app.api_url))
        .bearer_auth(&token)
        .json(&json!({ "provider_id": provider["id"], "prompt": "Say good morning via echo" }))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body: Value = response.json().await.unwrap();

    let _ = app
        .client
        .post(format!("{}/mcp-servers/{server_id}/disconnect", app.api_url))
        .bearer_auth(&token)
        .json(&json!({}))
        .send()
        .await;

    (status, body)
}

#[tokio::test]
#[ignore] // Requires uv and network access
async fn test_tools_with_llm_returns_final_answer_after_tool_call() {
    let (status, body) = run_test_tools(false, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["response"], "The tool said: good morning", "{body}");
    assert!(body.get("error").is_none(), "{body}");
    assert_eq!(body["tool_calls"][0]["name"], "echo");
    assert_eq!(body["tool_calls"][0]["result"]["content"][0]["text"], "good morning");

    // The LLM rounds are returned for the page's timeline
    let steps = body["llm_steps"].as_array().expect("llm_steps");
    assert_eq!(steps.len(), 2, "{body}");
    assert_eq!(steps[0]["request"]["new_messages"][0]["role"], "user");
    assert_eq!(steps[0]["response"]["tool_calls"][0]["name"], "echo");
    assert_eq!(steps[0]["tool_call_indices"], json!([0]));
    assert_eq!(steps[1]["request"]["new_messages"][1]["role"], "tool");
    assert_eq!(steps[1]["response"]["content"], "The tool said: good morning");
}

#[tokio::test]
#[ignore] // Requires uv and network access
async fn test_tools_with_llm_keeps_tool_calls_when_llm_is_rate_limited() {
    let (status, body) = run_test_tools(true, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["tool_calls"][0]["result"]["content"][0]["text"], "good morning", "{body}");
    assert!(body["error"].as_str().unwrap().contains("429"), "{body}");
    // The failed second round is part of the timeline
    let steps = body["llm_steps"].as_array().expect("llm_steps");
    assert_eq!(steps.len(), 2, "{body}");
    assert!(steps[1]["error"].as_str().unwrap().contains("429"), "{body}");
    assert!(steps[1]["response"].is_null());
}

#[tokio::test]
#[ignore] // Requires uv and network access
async fn test_tools_with_llm_caps_tool_result_sent_to_llm() {
    // 2 tokens ~ 8 characters: "good morning" reaches the LLM shortened
    let (status, body) = run_test_tools(false, Some(2)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let answer = body["response"].as_str().unwrap();
    assert!(answer.starts_with("The tool said: good mor\n\n[Tool result truncated to about 2 tokens"), "{answer}");
    // The full result is still returned for display
    assert_eq!(body["tool_calls"][0]["result"]["content"][0]["text"], "good morning");
}

/// Mock LLM that answers with the names of the tools it was offered.
async fn start_tool_listing_llm() -> String {
    let app = Router::new().route(
        "/v1/chat/completions",
        post(|Json(body): Json<Value>| async move {
            let names: Vec<String> = body["tools"]
                .as_array()
                .map(|tools| tools.iter().filter_map(|t| t["function"]["name"].as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            Json(json!({
                "id": "chatcmpl-mock",
                "object": "chat.completion",
                "created": 0,
                "model": body["model"],
                "choices": [{ "index": 0, "message": { "role": "assistant", "content": format!("tools: {}", names.join(",")) }, "finish_reason": "stop" }],
                "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
            }))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}/v1")
}

/// Ask the chat which tools it offers the LLM, as the given user.
/// Tool names the chat offers to the LLM; `switch_off` is a server switched
/// off for the new conversation before the message is sent.
async fn offered_tools(app: &common::TestApp, token: &str, llm: &str, switch_off: Option<i64>) -> String {
    let client = &app.client;
    client
        .post(format!("{}/llm-providers", app.api_url))
        .bearer_auth(token)
        .json(&json!({ "name": "Lister", "base_url": llm, "api_key": "k", "model": "mock-model", "is_default": true }))
        .send()
        .await
        .unwrap();
    let conversation: Value = client
        .post(format!("{}/conversations", app.api_url))
        .bearer_auth(token)
        .json(&json!({ "title": "tools?" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    if let Some(server_id) = switch_off {
        let off = client
            .put(format!("{}/conversations/{}/servers/{server_id}", app.api_url, conversation["id"]))
            .bearer_auth(token)
            .json(&json!({ "enabled": false }))
            .send()
            .await
            .unwrap();
        assert_eq!(off.status(), StatusCode::OK);
    }
    let message: Value = client
        .post(format!("{}/conversations/{}/messages", app.api_url, conversation["id"]))
        .bearer_auth(token)
        .json(&json!({ "content": "which tools?" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    message["content"].as_str().unwrap_or_default().to_string()
}

#[tokio::test]
#[ignore] // Requires uv and network access
async fn test_chat_offers_only_the_users_own_tools() {
    let app = setup_test_app().await;
    let owner = app.register_and_login("owner_tools", "owner_tools@example.com", "testpass123").await;
    let other = app.register_and_login("other_tools", "other_tools@example.com", "testpass123").await;

    // The owner connects the echo MCP server
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/echo_server_v2.py");
    let server: Value = app
        .client
        .post(format!("{}/mcp-servers", app.api_url))
        .bearer_auth(&owner)
        .json(&json!({
            "name": "echo",
            "server_type": "stdio",
            "config": { "command": "uv", "args": ["run", "--quiet", "--script", fixture], "env": {} },
            "use_container": false
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let server_id = server["id"].as_i64().unwrap();
    let connect = app
        .client
        .post(format!("{}/mcp-servers/{server_id}/connect", app.api_url))
        .bearer_auth(&owner)
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(connect.status(), StatusCode::OK);

    let llm = start_tool_listing_llm().await;
    let owner_sees = offered_tools(&app, &owner, &llm, None).await;
    let other_sees = offered_tools(&app, &other, &llm, None).await;
    // A conversation with the server switched off gets none of its tools
    let switched_off_sees = offered_tools(&app, &owner, &llm, Some(server_id)).await;

    let _ = app
        .client
        .post(format!("{}/mcp-servers/{server_id}/disconnect", app.api_url))
        .bearer_auth(&owner)
        .json(&json!({}))
        .send()
        .await;

    assert!(owner_sees.split(',').any(|t| t.trim_start_matches("tools: ") == "echo"), "owner: {owner_sees}");
    assert!(!other_sees.contains("echo"), "another user's chat must not see the owner's tools: {other_sees}");
    assert!(!switched_off_sees.contains("echo"), "a switched-off server must not offer tools: {switched_off_sees}");
}

#[tokio::test]
#[ignore] // Requires uv and network access
async fn test_chat_records_llm_steps_between_tool_calls() {
    let app = setup_test_app().await;
    let token = app.register_and_login("steps", "steps@example.com", "testpass123").await;
    let llm = start_mock_llm(false).await;
    app.client
        .post(format!("{}/llm-providers", app.api_url))
        .bearer_auth(&token)
        .json(&json!({ "name": "Mock", "base_url": llm, "api_key": "k", "model": "qwen3.8-27b-stable", "is_default": true }))
        .send()
        .await
        .unwrap();

    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/echo_server_v2.py");
    let server: Value = app
        .client
        .post(format!("{}/mcp-servers", app.api_url))
        .bearer_auth(&token)
        .json(&json!({
            "name": "echo",
            "server_type": "stdio",
            "config": { "command": "uv", "args": ["run", "--quiet", "--script", fixture], "env": {} },
            "use_container": false
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let server_id = server["id"].as_i64().unwrap();
    app.client
        .post(format!("{}/mcp-servers/{server_id}/connect", app.api_url))
        .bearer_auth(&token)
        .json(&json!({}))
        .send()
        .await
        .unwrap();

    let conversation: Value = app
        .client
        .post(format!("{}/conversations", app.api_url))
        .bearer_auth(&token)
        .json(&json!({ "title": "steps" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let message: Value = app
        .client
        .post(format!("{}/conversations/{}/messages", app.api_url, conversation["id"]))
        .bearer_auth(&token)
        .json(&json!({ "content": "Say good morning via echo" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let _ = app
        .client
        .post(format!("{}/mcp-servers/{server_id}/disconnect", app.api_url))
        .bearer_auth(&token)
        .json(&json!({}))
        .send()
        .await;

    assert_eq!(message["content"], "The tool said: good morning", "{message}");
    let steps: Value = serde_json::from_str(message["llm_steps"].as_str().expect("llm_steps")).unwrap();
    let steps = steps.as_array().unwrap();
    assert_eq!(steps.len(), 2, "{steps:?}");

    // Round 1: the user message goes out, the LLM asks for echo
    assert_eq!(steps[0]["round"], 1);
    assert_eq!(steps[0]["request"]["new_messages"][0]["role"], "user");
    assert!(steps[0]["request"]["tools"].as_array().unwrap().iter().any(|t| t == "echo"));
    assert_eq!(steps[0]["response"]["tool_calls"][0]["name"], "echo");
    assert_eq!(steps[0]["tool_call_indices"], json!([0]));
    assert!(steps[0]["usage"]["total_tokens"].is_number());

    // Round 2: the assistant tool call and the tool result go out, the answer comes back
    let roles: Vec<&str> = steps[1]["request"]["new_messages"].as_array().unwrap().iter().map(|m| m["role"].as_str().unwrap()).collect();
    assert_eq!(roles, vec!["assistant", "tool"]);
    assert_eq!(steps[1]["request"]["new_messages"][1]["content"], "good morning");
    assert_eq!(steps[1]["response"]["content"], "The tool said: good morning");
    assert_eq!(steps[1]["tool_call_indices"], json!([]));
}

/// Mock LLM that asks for `echo` in every round and never answers
async fn start_looping_llm() -> String {
    let app = Router::new().route(
        "/v1/chat/completions",
        post(|Json(body): Json<Value>| async move {
            let round = body["messages"].as_array().map(|m| m.len()).unwrap_or(0);
            Json(json!({
                "id": "chatcmpl-mock",
                "object": "chat.completion",
                "created": 0,
                "model": body["model"],
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": format!("call_{round}"),
                            "type": "function",
                            "function": { "name": "echo", "arguments": format!("{{\"text\":\"round {round}\"}}") }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }],
                "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
            }))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}/v1")
}

#[tokio::test]
#[ignore] // Requires uv and network access
async fn test_chat_saves_rounds_when_the_round_limit_is_reached() {
    let app = setup_test_app().await;
    let token = app.register_and_login("loop", "loop@example.com", "testpass123").await;
    let llm = start_looping_llm().await;
    app.client
        .post(format!("{}/llm-providers", app.api_url))
        .bearer_auth(&token)
        .json(&json!({ "name": "Loop", "base_url": llm, "api_key": "k", "model": "mock-model", "is_default": true }))
        .send()
        .await
        .unwrap();

    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/echo_server_v2.py");
    let server: Value = app
        .client
        .post(format!("{}/mcp-servers", app.api_url))
        .bearer_auth(&token)
        .json(&json!({
            "name": "echo",
            "server_type": "stdio",
            "config": { "command": "uv", "args": ["run", "--quiet", "--script", fixture], "env": {} },
            "use_container": false
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let server_id = server["id"].as_i64().unwrap();
    let connect = app
        .client
        .post(format!("{}/mcp-servers/{server_id}/connect", app.api_url))
        .bearer_auth(&token)
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(connect.status(), StatusCode::OK);

    let conversation: Value = app
        .client
        .post(format!("{}/conversations", app.api_url))
        .bearer_auth(&token)
        .json(&json!({ "title": "loop" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let response = app
        .client
        .post(format!("{}/conversations/{}/messages", app.api_url, conversation["id"]))
        .bearer_auth(&token)
        .json(&json!({ "content": "keep going" }))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let message: Value = response.json().await.unwrap();

    let _ = app
        .client
        .post(format!("{}/mcp-servers/{server_id}/disconnect", app.api_url))
        .bearer_auth(&token)
        .json(&json!({}))
        .send()
        .await;

    // Not an internal error: the rounds are saved with a clear note
    assert_eq!(status, StatusCode::OK, "{message}");
    assert_eq!(message["content"], "Stopped after 10 LLM rounds without a final answer.");
    let steps: Vec<Value> = serde_json::from_str(message["llm_steps"].as_str().unwrap()).unwrap();
    assert_eq!(steps.len(), 10);
    let calls: Vec<Value> = serde_json::from_str(message["tool_calls"].as_str().unwrap()).unwrap();
    assert_eq!(calls.len(), 10);
}
