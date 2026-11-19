//! Chat with a mock OpenAI-compatible LLM, over the REST endpoint and the
//! WebSocket the web UI uses. No MCP server needed.

use axum::{routing::post, Json, Router};
use futures::{SinkExt, StreamExt};
use reqwest::StatusCode;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message as WsMessage};

mod common;
use common::{setup_test_app, TestApp};

/// Mock LLM that answers like LM Studio: the final text together with an
/// empty "tool_calls" list. Returns the base URL and a request counter.
async fn start_lm_studio_like_llm() -> (String, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move |Json(body): Json<Value>| {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Json(json!({
                    "id": "chatcmpl-mock",
                    "object": "chat.completion",
                    "created": 0,
                    "model": body["model"],
                    "choices": [{
                        "index": 0,
                        "message": { "role": "assistant", "content": "Final answer", "tool_calls": [] },
                        "finish_reason": "stop"
                    }],
                    "usage": { "prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7 }
                }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}/v1"), calls)
}

async fn add_provider(app: &TestApp, token: &str, llm: &str, name: &str, is_default: bool) -> i64 {
    let provider = app
        .client
        .post(format!("{}/llm-providers", app.api_url))
        .bearer_auth(token)
        .json(&json!({ "name": name, "base_url": llm, "api_key": "k", "model": "google/gemma-4-e4b", "is_default": is_default }))
        .send()
        .await
        .unwrap();
    assert_eq!(provider.status(), StatusCode::CREATED);
    provider.json::<Value>().await.unwrap()["id"].as_i64().unwrap()
}

/// A user with the mock LLM as default provider and one conversation
async fn user_with_conversation(app: &TestApp, llm: &str) -> (String, i64) {
    let token = app.register_and_login("chat", "chat@example.com", "testpass123").await;
    add_provider(app, &token, llm, "LM Studio", true).await;
    let conversation: Value = app
        .client
        .post(format!("{}/conversations", app.api_url))
        .bearer_auth(&token)
        .json(&json!({ "title": "notes" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    (token, conversation["id"].as_i64().unwrap())
}

#[tokio::test]
async fn rest_chat_takes_an_answer_with_empty_tool_calls_as_final() {
    let app = setup_test_app().await;
    let (llm, calls) = start_lm_studio_like_llm().await;
    let (token, conversation_id) = user_with_conversation(&app, &llm).await;

    let response = app
        .client
        .post(format!("{}/conversations/{conversation_id}/messages", app.api_url))
        .bearer_auth(&token)
        .json(&json!({ "content": "list my last three notes" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let message: Value = response.json().await.unwrap();
    assert_eq!(message["content"], "Final answer");
    let steps: Vec<Value> = serde_json::from_str(message["llm_steps"].as_str().unwrap()).unwrap();
    assert_eq!(steps.len(), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1, "the LLM must be asked once");
}

#[tokio::test]
async fn websocket_chat_takes_an_answer_with_empty_tool_calls_as_final() {
    let app = setup_test_app().await;
    let (llm, calls) = start_lm_studio_like_llm().await;
    let (token, conversation_id) = user_with_conversation(&app, &llm).await;

    let url = format!("{}/ws/conversations/{conversation_id}", app.api_url.replacen("http", "ws", 1));
    let mut request = url.into_client_request().unwrap();
    request.headers_mut().insert("Authorization", format!("Bearer {token}").parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    socket
        .send(WsMessage::Text(json!({ "content": "list my last three notes" }).to_string()))
        .await
        .unwrap();

    // Read events until the chat ends, with a timeout so a hang fails the test
    let last = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while let Some(frame) = socket.next().await {
            if let WsMessage::Text(text) = frame.unwrap() {
                let event: Value = serde_json::from_str(&text).unwrap();
                if event["type"] == "assistant_complete" || event["type"] == "error" {
                    return event;
                }
            }
        }
        panic!("WebSocket closed before the chat ended");
    })
    .await
    .expect("chat did not finish");

    assert_eq!(last["type"], "assistant_complete", "{last}");
    assert_eq!(last["content"], "Final answer");
    assert_eq!(last["llm_steps"].as_array().unwrap().len(), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1, "the LLM must be asked once");

    // The WebSocket chat remembers the provider too (here the default one)
    let conversation = get_conversation(&app, &token, conversation_id).await;
    assert!(conversation["last_provider_id"].is_number(), "{conversation}");
}

async fn get_conversation(app: &TestApp, token: &str, conversation_id: i64) -> Value {
    app.client
        .get(format!("{}/conversations/{conversation_id}", app.api_url))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

/// The chat page with the API token as session cookie (both are the same JWT)
async fn chat_page(app: &TestApp, token: &str, conversation_id: i64) -> String {
    let base = app.api_url.trim_end_matches("/api/v1");
    let response = app
        .client
        .get(format!("{base}/chat/{conversation_id}"))
        .header(reqwest::header::COOKIE, format!("mcp_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.text().await.unwrap()
}

#[tokio::test]
async fn chat_preselects_the_last_used_provider_and_falls_back_to_the_default() {
    let app = setup_test_app().await;
    let (llm, _) = start_lm_studio_like_llm().await;
    let (token, conversation_id) = user_with_conversation(&app, &llm).await;
    let default_id = get_providers(&app, &token).await[0]["id"].as_i64().unwrap();
    let other_id = add_provider(&app, &token, &llm, "Other", false).await;
    let selected = |id: i64| format!("<option value=\"{id}\" selected>");

    // Nothing sent yet: the default provider
    assert_eq!(get_conversation(&app, &token, conversation_id).await["last_provider_id"], Value::Null);
    let html = chat_page(&app, &token, conversation_id).await;
    assert!(html.contains(&selected(default_id)) && !html.contains(&selected(other_id)), "{html}");

    // A message with the other provider: it is remembered and preselected
    let sent = app
        .client
        .post(format!("{}/conversations/{conversation_id}/messages", app.api_url))
        .bearer_auth(&token)
        .json(&json!({ "content": "hi", "provider_id": other_id }))
        .send()
        .await
        .unwrap();
    assert_eq!(sent.status(), StatusCode::OK);
    assert_eq!(get_conversation(&app, &token, conversation_id).await["last_provider_id"], other_id);
    let html = chat_page(&app, &token, conversation_id).await;
    assert!(html.contains(&selected(other_id)) && !html.contains(&selected(default_id)), "{html}");

    // The provider is deleted: back to the default
    let deleted = app
        .client
        .delete(format!("{}/llm-providers/{other_id}", app.api_url))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert!(deleted.status().is_success());
    let html = chat_page(&app, &token, conversation_id).await;
    assert!(html.contains(&selected(default_id)), "{html}");
}

async fn get_providers(app: &TestApp, token: &str) -> Vec<Value> {
    app.client
        .get(format!("{}/llm-providers", app.api_url))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}
