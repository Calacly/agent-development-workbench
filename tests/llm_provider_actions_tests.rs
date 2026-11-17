//! Tests for the model listing and "hello" test endpoints of LLM providers.
//! The app starts in the test process (see tests/common); the LLM itself is
//! a small mock OpenAI-compatible server started per test.

use axum::{
    http::{HeaderMap, StatusCode as AxumStatus},
    routing::{get, post},
    Json, Router,
};
use reqwest::StatusCode;
use serde_json::{json, Value};

mod common;
use common::{setup_test_app, TestApp};

const MOCK_KEY: &str = "sk-mock-key";

/// Start a mock OpenAI-compatible API and return its base URL (".../v1").
async fn start_mock_llm() -> String {
    fn authorized(headers: &HeaderMap) -> bool {
        headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            == Some(&format!("Bearer {MOCK_KEY}"))
    }

    let app = Router::new()
        .route(
            "/v1/models",
            get(|headers: HeaderMap| async move {
                if !authorized(&headers) {
                    return Err(AxumStatus::UNAUTHORIZED);
                }
                Ok(Json(json!({
                    "object": "list",
                    "data": [{ "id": "mock-b" }, { "id": "mock-a" }, { "id": "mock-b" }]
                })))
            }),
        )
        .route(
            "/v1/chat/completions",
            post(|headers: HeaderMap, Json(body): Json<Value>| async move {
                if !authorized(&headers) {
                    return Err(AxumStatus::UNAUTHORIZED);
                }
                let prompt = body["messages"][0]["content"].as_str().unwrap_or_default().to_string();
                // Same rate-limit headers Groq and OpenAI send
                let limits = [
                    ("x-ratelimit-limit-tokens", "8000"),
                    ("x-ratelimit-remaining-tokens", "7990"),
                ];
                Ok((limits, Json(json!({
                    "id": "chatcmpl-mock",
                    "object": "chat.completion",
                    "created": 0,
                    "model": body["model"],
                    "choices": [{
                        "index": 0,
                        "message": { "role": "assistant", "content": format!("Hi! You said: {prompt}") },
                        "finish_reason": "stop"
                    }],
                    "usage": { "prompt_tokens": 1, "completion_tokens": 4, "total_tokens": 5 }
                }))))
            }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}/v1")
}

/// A base URL on which nothing listens.
async fn unreachable_base_url() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    format!("http://{addr}/v1")
}

async fn create_provider(app: &TestApp, token: &str, base_url: &str, model: Option<&str>) -> i64 {
    let response = app
        .client
        .post(format!("{}/llm-providers", app.api_url))
        .bearer_auth(token)
        .json(&json!({
            "name": "Mock",
            "base_url": base_url,
            "api_key": MOCK_KEY,
            "model": model,
            "is_default": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    response.json::<Value>().await.unwrap()["id"].as_i64().unwrap()
}

#[tokio::test]
async fn list_models_for_unsaved_config_is_sorted_and_deduplicated() {
    let app = setup_test_app().await;
    let token = app.register_and_login("models_cfg", "models_cfg@example.com", "testpass123").await;
    let mock = start_mock_llm().await;

    let response = app
        .client
        .post(format!("{}/llm-providers/models", app.api_url))
        .bearer_auth(&token)
        .json(&json!({ "base_url": format!("{mock}/"), "api_key": MOCK_KEY }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["models"], json!(["mock-a", "mock-b"]));
}

#[tokio::test]
async fn list_models_with_wrong_key_is_bad_request() {
    let app = setup_test_app().await;
    let token = app.register_and_login("models_key", "models_key@example.com", "testpass123").await;
    let mock = start_mock_llm().await;

    let response = app
        .client
        .post(format!("{}/llm-providers/models", app.api_url))
        .bearer_auth(&token)
        .json(&json!({ "base_url": mock, "api_key": "wrong" }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn list_models_requires_auth() {
    let app = setup_test_app().await;
    let response = app
        .client
        .post(format!("{}/llm-providers/models", app.api_url))
        .json(&json!({ "base_url": "http://127.0.0.1:1/v1" }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_of_other_users_provider_is_not_found() {
    let app = setup_test_app().await;
    let owner = app.register_and_login("owner", "owner@example.com", "testpass123").await;
    let other = app.register_and_login("other", "other@example.com", "testpass123").await;
    let mock = start_mock_llm().await;
    let id = create_provider(&app, &owner, &mock, Some("mock-a")).await;

    let response = app
        .client
        .post(format!("{}/llm-providers/{id}/test", app.api_url))
        .bearer_auth(&other)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_unsaved_config_sends_hello_and_returns_reply() {
    let app = setup_test_app().await;
    let token = app.register_and_login("hello_cfg", "hello_cfg@example.com", "testpass123").await;
    let mock = start_mock_llm().await;

    let response = app
        .client
        .post(format!("{}/llm-providers/test", app.api_url))
        .bearer_auth(&token)
        .json(&json!({ "base_url": mock, "api_key": MOCK_KEY, "model": "mock-b" }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["success"], true, "{body}");
    assert_eq!(body["reply"], "Hi! You said: hello");
    assert_eq!(body["model"], "mock-b");
}

#[tokio::test]
async fn test_unsaved_config_with_wrong_key_reports_failure() {
    let app = setup_test_app().await;
    let token = app.register_and_login("hello_bad", "hello_bad@example.com", "testpass123").await;
    let mock = start_mock_llm().await;

    let response = app
        .client
        .post(format!("{}/llm-providers/test", app.api_url))
        .bearer_auth(&token)
        .json(&json!({ "base_url": mock, "api_key": "wrong", "model": "mock-a" }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["success"], false);
    assert!(body["message"].as_str().unwrap().contains("401"), "{body}");
}

#[tokio::test]
async fn test_unsaved_config_requires_auth() {
    let app = setup_test_app().await;
    let response = app
        .client
        .post(format!("{}/llm-providers/test", app.api_url))
        .json(&json!({ "base_url": "http://127.0.0.1:1/v1", "model": "x" }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_connection_sends_hello_and_returns_reply() {
    let app = setup_test_app().await;
    let token = app.register_and_login("hello", "hello@example.com", "testpass123").await;
    let mock = start_mock_llm().await;
    let id = create_provider(&app, &token, &mock, Some("mock-a")).await;

    let response = app
        .client
        .post(format!("{}/llm-providers/{id}/test", app.api_url))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["success"], true, "{body}");
    assert_eq!(body["reply"], "Hi! You said: hello");
    assert_eq!(body["model"], "mock-a");
    assert!(body["latency_ms"].is_u64());
}

#[tokio::test]
async fn test_connection_without_model_fails_cleanly() {
    let app = setup_test_app().await;
    let token = app.register_and_login("nomodel", "nomodel@example.com", "testpass123").await;
    let mock = start_mock_llm().await;
    let id = create_provider(&app, &token, &mock, None).await;

    let response = app
        .client
        .post(format!("{}/llm-providers/{id}/test", app.api_url))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["success"], false);
    assert_eq!(body["message"], "No model configured");
}

#[tokio::test]
async fn test_connection_to_unreachable_provider_reports_failure() {
    let app = setup_test_app().await;
    let token = app.register_and_login("down", "down@example.com", "testpass123").await;
    let base = unreachable_base_url().await;
    let id = create_provider(&app, &token, &base, Some("mock-a")).await;

    let response = app
        .client
        .post(format!("{}/llm-providers/{id}/test", app.api_url))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["success"], false);
    assert!(body["message"].as_str().unwrap().contains("Failed to send request"), "{body}");
}

async fn get_provider(app: &TestApp, token: &str, id: i64) -> Value {
    app.client
        .get(format!("{}/llm-providers/{id}", app.api_url))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test]
async fn test_connection_records_token_limit_and_cap_can_be_set() {
    let app = setup_test_app().await;
    let token = app.register_and_login("limits", "limits@example.com", "testpass123").await;
    let mock = start_mock_llm().await;
    let id = create_provider(&app, &token, &mock, Some("mock-a")).await;

    let before = get_provider(&app, &token, id).await;
    assert!(before["rate_limit_tokens"].is_null(), "{before}");
    assert!(before["tool_result_token_cap"].is_null(), "no limit known yet, so no cap: {before}");

    let test: Value = app
        .client
        .post(format!("{}/llm-providers/{id}/test", app.api_url))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(test["rate_limit_tokens"], 8000, "{test}");
    assert_eq!(test["remaining_tokens"], 7990, "{test}");

    let after = get_provider(&app, &token, id).await;
    assert_eq!(after["rate_limit_tokens"], 8000, "{after}");
    assert!(after["rate_limit_checked_at"].is_string(), "{after}");
    assert_eq!(after["tool_result_token_cap"], 4000, "automatic cap is half the limit: {after}");

    let put = |tokens: i64| {
        app.client
            .put(format!("{}/llm-providers/{id}", app.api_url))
            .bearer_auth(&token)
            .json(&json!({ "max_tool_result_tokens": tokens }))
            .send()
    };
    let set: Value = put(1000).await.unwrap().json().await.unwrap();
    assert_eq!(set["max_tool_result_tokens"], 1000, "{set}");
    assert_eq!(set["tool_result_token_cap"], 1000, "{set}");

    let cleared: Value = put(0).await.unwrap().json().await.unwrap();
    assert!(cleared["max_tool_result_tokens"].is_null(), "{cleared}");
    assert_eq!(cleared["tool_result_token_cap"], 4000, "{cleared}");

    assert_eq!(put(-5).await.unwrap().status(), StatusCode::BAD_REQUEST);
}

/// Providers without an API key (local Ollama, LM Studio) must work in the chat
#[tokio::test]
async fn chat_works_with_a_provider_without_api_key() {
    let app = setup_test_app().await;
    let token = app.register_and_login("nokey", "nokey@example.com", "testpass123").await;

    // Local-style mock: no Authorization check
    let mock = Router::new().route(
        "/v1/chat/completions",
        post(|Json(body): Json<Value>| async move {
            Json(json!({
                "id": "x", "object": "chat.completion", "created": 0, "model": body["model"],
                "choices": [{ "index": 0, "message": { "role": "assistant", "content": "local answer" }, "finish_reason": "stop" }],
                "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
            }))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });

    let created = app
        .client
        .post(format!("{}/llm-providers", app.api_url))
        .bearer_auth(&token)
        .json(&json!({ "name": "Local", "base_url": format!("http://{addr}/v1"), "api_key": "", "model": "llama3.2", "is_default": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);

    let conversation: Value = app
        .client
        .post(format!("{}/conversations", app.api_url))
        .bearer_auth(&token)
        .json(&json!({ "title": "local" }))
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
        .json(&json!({ "content": "hi" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "{}", response.text().await.unwrap());
    let message: Value = response.json().await.unwrap();
    assert_eq!(message["content"], "local answer");
}
