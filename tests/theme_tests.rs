//! POST /settings/generate-theme with mock OpenAI-compatible LLMs

use axum::{routing::post, Json, Router};
use reqwest::StatusCode;
use serde_json::{json, Value};

mod common;
use common::{setup_test_app, TestApp};

const THEME: &str = r##"{"name":"Sunset","mode":"dark","colors":{"primary":"#d8573a","secondary":"#72409e","accent":"#ff8c3d","neutral":"#b0a99b","base_100":"#150e23","base_200":"#281a3c","base_300":"#4b2e58","info":"#3abff8","success":"#36d399","warning":"#fbbd23","error":"#f87272"}}"##;

/// Mock LLM that always answers with `content` and `finish_reason`
async fn start_llm(content: String, finish_reason: &'static str) -> String {
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move |Json(body): Json<Value>| {
            let content = content.clone();
            async move {
                Json(json!({
                    "id": "chatcmpl-mock",
                    "object": "chat.completion",
                    "created": 0,
                    "model": body["model"],
                    "choices": [{ "index": 0, "message": { "role": "assistant", "content": content }, "finish_reason": finish_reason }],
                    "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
                }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}/v1")
}

async fn add_provider(app: &TestApp, token: &str, llm: &str, name: &str, is_default: bool) -> i64 {
    let response = app
        .client
        .post(format!("{}/llm-providers", app.api_url))
        .bearer_auth(token)
        .json(&json!({ "name": name, "base_url": llm, "api_key": "k", "model": "mock", "is_default": is_default }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    response.json::<Value>().await.unwrap()["id"].as_i64().unwrap()
}

async fn generate(app: &TestApp, token: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .client
        .post(format!("{}/settings/generate-theme", app.api_url))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .unwrap();
    (response.status(), response.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn generate_theme_uses_the_chosen_provider_and_reads_fenced_json() {
    let app = setup_test_app().await;
    let token = app.register_and_login("theme", "theme@example.com", "testpass123").await;
    // The default provider answers with prose only; the chosen one wraps the JSON in a fence
    let broken = start_llm("I cannot do that.".into(), "stop").await;
    let fenced = start_llm(format!("Here is your theme:\n```json\n{THEME}\n```"), "stop").await;
    add_provider(&app, &token, &broken, "Default", true).await;
    let chosen = add_provider(&app, &token, &fenced, "Chosen", false).await;

    let (status, body) = generate(&app, &token, json!({ "prompt": "sunset", "provider_id": chosen })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["theme"]["name"], "[custom] Sunset");
    assert_eq!(body["theme"]["colors"]["primary"], "#d8573a");

    // Without provider_id the default provider is used
    let (status, body) = generate(&app, &token, json!({ "prompt": "sunset" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["details"].as_str().unwrap().contains("did not return a valid theme"), "{body}");
}

#[tokio::test]
async fn generate_theme_explains_a_cut_off_answer() {
    let app = setup_test_app().await;
    let token = app.register_and_login("theme", "theme@example.com", "testpass123").await;
    let cut_off = start_llm(THEME[..60].to_string(), "length").await;
    add_provider(&app, &token, &cut_off, "Thinker", true).await;

    let (status, body) = generate(&app, &token, json!({ "prompt": "sunset" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["details"].as_str().unwrap().contains("token limit"), "{body}");
}

#[tokio::test]
async fn generate_theme_rejects_missing_and_foreign_providers() {
    let app = setup_test_app().await;
    let owner = app.register_and_login("owner", "owner@example.com", "testpass123").await;
    let other = app.register_and_login("other", "other@example.com", "testpass123").await;
    let llm = start_llm(THEME.to_string(), "stop").await;
    let owners_provider = add_provider(&app, &owner, &llm, "Mine", true).await;

    // No provider at all
    let (status, body) = generate(&app, &other, json!({ "prompt": "sunset" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["details"].as_str().unwrap().contains("no default"), "{body}");
    // Another user's provider
    let (status, _) = generate(&app, &other, json!({ "prompt": "sunset", "provider_id": owners_provider })).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn settings_page_offers_the_providers_with_the_default_selected() {
    let app = setup_test_app().await;
    let token = app.register_and_login("theme", "theme@example.com", "testpass123").await;
    let base = app.api_url.trim_end_matches("/api/v1").to_string();
    let page = |token: String| {
        let client = app.client.clone();
        let base = base.clone();
        async move {
            client
                .get(format!("{base}/settings"))
                .header(reqwest::header::COOKIE, format!("mcp_session={token}"))
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap()
        }
    };

    let html = page(token.clone()).await;
    assert!(html.contains("No LLM provider configured"), "{html}");

    let llm = start_llm(THEME.to_string(), "stop").await;
    add_provider(&app, &token, &llm, "Other", false).await;
    let default_id = add_provider(&app, &token, &llm, "Main", true).await;
    let html = page(token).await;
    assert!(html.contains(r#"name="provider_id""#), "{html}");
    assert!(html.contains(&format!(r#"<option value="{default_id}" selected>"#)), "{html}");
}
