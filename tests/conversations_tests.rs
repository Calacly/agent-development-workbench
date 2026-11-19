use reqwest::StatusCode;
use serde_json::json;

mod common;
use common::setup_test_app;

#[tokio::test]
async fn test_create_conversation() {
    let app = setup_test_app().await;
    let token = app.register_and_login("testuser", "test@example.com", "testpass123").await;

    let response = app
        .client
        .post(&format!("{}/conversations", app.api_url))
        .bearer_auth(&token)
        .json(&json!({
            "title": "Test Conversation"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);

    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["title"], "Test Conversation");
    assert!(body["id"].is_number());
    assert!(body["created_at"].is_string());
    assert!(body["updated_at"].is_string());
}

#[tokio::test]
async fn test_list_conversations() {
    let app = setup_test_app().await;
    let token = app.register_and_login("testuser", "test@example.com", "testpass123").await;

    // Create two conversations
    for i in 1..=2 {
        app.client
            .post(&format!("{}/conversations", app.api_url))
            .bearer_auth(&token)
            .json(&json!({
                "title": format!("Conversation {}", i)
            }))
            .send()
            .await
            .unwrap();
    }

    let response = app
        .client
        .get(&format!("{}/conversations", app.api_url))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body: Vec<serde_json::Value> = response.json().await.unwrap();
    assert!(body.len() >= 2);
}

#[tokio::test]
async fn test_get_conversation() {
    let app = setup_test_app().await;
    let token = app.register_and_login("testuser", "test@example.com", "testpass123").await;

    // Create a conversation
    let create_response = app
        .client
        .post(&format!("{}/conversations", app.api_url))
        .bearer_auth(&token)
        .json(&json!({
            "title": "Test Conversation"
        }))
        .send()
        .await
        .unwrap();

    let created: serde_json::Value = create_response.json().await.unwrap();
    let conversation_id = created["id"].as_i64().unwrap();

    // Get the conversation
    let response = app
        .client
        .get(&format!("{}/conversations/{}", app.api_url, conversation_id))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["title"], "Test Conversation");
    assert_eq!(body["id"], conversation_id);
}

#[tokio::test]
async fn test_delete_conversation() {
    let app = setup_test_app().await;
    let token = app.register_and_login("testuser", "test@example.com", "testpass123").await;

    // Create a conversation
    let create_response = app
        .client
        .post(&format!("{}/conversations", app.api_url))
        .bearer_auth(&token)
        .json(&json!({
            "title": "To Delete"
        }))
        .send()
        .await
        .unwrap();

    let created: serde_json::Value = create_response.json().await.unwrap();
    let conversation_id = created["id"].as_i64().unwrap();

    // Delete the conversation
    let response = app
        .client
        .delete(&format!("{}/conversations/{}", app.api_url, conversation_id))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Verify it's deleted
    let get_response = app
        .client
        .get(&format!("{}/conversations/{}", app.api_url, conversation_id))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();

    assert_eq!(get_response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_list_messages_empty_conversation() {
    let app = setup_test_app().await;
    let token = app.register_and_login("testuser", "test@example.com", "testpass123").await;

    // Create a conversation
    let conversation = app
        .client
        .post(&format!("{}/conversations", app.api_url))
        .bearer_auth(&token)
        .json(&json!({
            "title": "Empty Conversation"
        }))
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();

    let conversation_id = conversation["id"].as_i64().unwrap();

    // List messages
    let response = app
        .client
        .get(&format!("{}/conversations/{}/messages", app.api_url, conversation_id))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body: Vec<serde_json::Value> = response.json().await.unwrap();
    assert_eq!(body.len(), 0);
}

#[tokio::test]
async fn test_send_message_without_provider() {
    let app = setup_test_app().await;
    let token = app.register_and_login("testuser", "test@example.com", "testpass123").await;

    // Create a conversation
    let conversation = app
        .client
        .post(&format!("{}/conversations", app.api_url))
        .bearer_auth(&token)
        .json(&json!({
            "title": "Test Chat"
        }))
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();

    let conversation_id = conversation["id"].as_i64().unwrap();

    // Try to send message without LLM provider configured
    let response = app
        .client
        .post(&format!("{}/conversations/{}/messages", app.api_url, conversation_id))
        .bearer_auth(&token)
        .json(&json!({
            "content": "Hello!"
        }))
        .send()
        .await
        .unwrap();

    // Should fail because no provider is configured
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_conversation_isolation_between_users() {
    let app = setup_test_app().await;
    let token1 = app.register_and_login("user1", "user1@example.com", "password123").await;
    let token2 = app.register_and_login("user2", "user2@example.com", "password123").await;

    // User 1 creates a conversation
    let conversation = app
        .client
        .post(&format!("{}/conversations", app.api_url))
        .bearer_auth(&token1)
        .json(&json!({
            "title": "User 1 Conversation"
        }))
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();

    let conversation_id = conversation["id"].as_i64().unwrap();

    // User 2 tries to access user 1's conversation
    let response = app
        .client
        .get(&format!("{}/conversations/{}", app.api_url, conversation_id))
        .bearer_auth(&token2)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // User 2 tries to send message to user 1's conversation
    let message_response = app
        .client
        .post(&format!("{}/conversations/{}/messages", app.api_url, conversation_id))
        .bearer_auth(&token2)
        .json(&json!({
            "content": "Hello!"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(message_response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_unauthorized_access_to_conversations() {
    let app = setup_test_app().await;

    let response = app
        .client
        .get(&format!("{}/conversations", app.api_url))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_delete_conversation_cascades_messages() {
    let app = setup_test_app().await;
    let token = app.register_and_login("testuser", "test@example.com", "testpass123").await;

    // Create conversation
    let conversation = app
        .client
        .post(&format!("{}/conversations", app.api_url))
        .bearer_auth(&token)
        .json(&json!({
            "title": "Test Cascade"
        }))
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();

    let conversation_id = conversation["id"].as_i64().unwrap();

    // Delete conversation
    let delete_response = app
        .client
        .delete(&format!("{}/conversations/{}", app.api_url, conversation_id))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();

    assert_eq!(delete_response.status(), StatusCode::NO_CONTENT);

    // Try to list messages - should get 404 because conversation is gone
    let messages_response = app
        .client
        .get(&format!("{}/conversations/{}/messages", app.api_url, conversation_id))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();

    assert_eq!(messages_response.status(), StatusCode::NOT_FOUND);
}

async fn create_conversation(app: &common::TestApp, token: &str, title: &str) -> i64 {
    let response = app
        .client
        .post(format!("{}/conversations", app.api_url))
        .bearer_auth(token)
        .json(&json!({ "title": title }))
        .send()
        .await
        .unwrap();
    response.json::<serde_json::Value>().await.unwrap()["id"].as_i64().unwrap()
}

async fn create_server(app: &common::TestApp, token: &str, name: &str) -> i64 {
    let response = app
        .client
        .post(format!("{}/mcp-servers", app.api_url))
        .bearer_auth(token)
        .json(&json!({
            "name": name,
            "server_type": "stdio",
            "config": { "command": "uvx", "args": ["mcp-server-git"], "env": {} }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    response.json::<serde_json::Value>().await.unwrap()["id"].as_i64().unwrap()
}

async fn conversation_servers(app: &common::TestApp, token: &str, conversation_id: i64) -> Vec<serde_json::Value> {
    let response = app
        .client
        .get(format!("{}/conversations/{}/servers", app.api_url, conversation_id))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.json().await.unwrap()
}

async fn set_server(app: &common::TestApp, token: &str, conversation_id: i64, server_id: i64, enabled: bool) -> reqwest::Response {
    app.client
        .put(format!("{}/conversations/{}/servers/{}", app.api_url, conversation_id, server_id))
        .bearer_auth(token)
        .json(&json!({ "enabled": enabled }))
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn test_conversation_servers_are_on_by_default_and_switch_per_conversation() {
    let app = setup_test_app().await;
    let token = app.register_and_login("testuser", "test@example.com", "testpass123").await;
    let first = create_conversation(&app, &token, "First").await;
    let second = create_conversation(&app, &token, "Second").await;
    let server_id = create_server(&app, &token, "Git").await;

    let servers = conversation_servers(&app, &token, first).await;
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0]["id"], server_id);
    assert_eq!(servers[0]["name"], "Git");
    assert_eq!(servers[0]["enabled"], true);
    assert_eq!(servers[0]["connected"], false);

    let off = set_server(&app, &token, first, server_id, false).await;
    assert_eq!(off.status(), StatusCode::OK);
    assert_eq!(off.json::<serde_json::Value>().await.unwrap()["enabled"], false);
    assert_eq!(conversation_servers(&app, &token, first).await[0]["enabled"], false);
    // Switching off twice is fine
    assert_eq!(set_server(&app, &token, first, server_id, false).await.status(), StatusCode::OK);
    // The other conversation keeps the server on
    assert_eq!(conversation_servers(&app, &token, second).await[0]["enabled"], true);

    assert_eq!(set_server(&app, &token, first, server_id, true).await.status(), StatusCode::OK);
    assert_eq!(conversation_servers(&app, &token, first).await[0]["enabled"], true);

    // A deleted server disappears from the list
    set_server(&app, &token, first, server_id, false).await;
    app.client
        .delete(format!("{}/mcp-servers/{}", app.api_url, server_id))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert!(conversation_servers(&app, &token, first).await.is_empty());
}

#[tokio::test]
async fn test_conversation_servers_stay_private() {
    let app = setup_test_app().await;
    let owner = app.register_and_login("owner", "owner@example.com", "testpass123").await;
    let other = app.register_and_login("other", "other@example.com", "testpass123").await;
    let conversation_id = create_conversation(&app, &owner, "Mine").await;
    let server_id = create_server(&app, &owner, "Git").await;
    let other_conversation = create_conversation(&app, &other, "Theirs").await;

    // Another user can neither read nor change the switches of this conversation
    let list = app
        .client
        .get(format!("{}/conversations/{}/servers", app.api_url, conversation_id))
        .bearer_auth(&other)
        .send()
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::NOT_FOUND);
    assert_eq!(set_server(&app, &other, conversation_id, server_id, false).await.status(), StatusCode::NOT_FOUND);
    // nor switch the owner's server in their own conversation
    assert_eq!(set_server(&app, &other, other_conversation, server_id, false).await.status(), StatusCode::NOT_FOUND);
    // and never see the owner's servers
    assert!(conversation_servers(&app, &other, other_conversation).await.is_empty());

    // Unknown server
    assert_eq!(set_server(&app, &owner, conversation_id, 999_999, false).await.status(), StatusCode::NOT_FOUND);
    // Without a token
    let anonymous = app
        .client
        .get(format!("{}/conversations/{}/servers", app.api_url, conversation_id))
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_delete_conversation_of_another_user_keeps_its_messages() {
    let app = setup_test_app().await;
    let owner = app.register_and_login("owner", "owner@example.com", "testpass123").await;
    let other = app.register_and_login("other", "other@example.com", "testpass123").await;
    let conversation_id = create_conversation(&app, &owner, "Private").await;

    // Without an LLM provider the request fails, but the user message is saved
    app.client
        .post(format!("{}/conversations/{}/messages", app.api_url, conversation_id))
        .bearer_auth(&owner)
        .json(&json!({ "content": "keep me" }))
        .send()
        .await
        .unwrap();
    let messages = |token: String| {
        let client = app.client.clone();
        let url = format!("{}/conversations/{}/messages", app.api_url, conversation_id);
        async move { client.get(url).bearer_auth(token).send().await.unwrap().json::<Vec<serde_json::Value>>().await.unwrap() }
    };
    assert_eq!(messages(owner.clone()).await.len(), 1);

    let foreign = app
        .client
        .delete(format!("{}/conversations/{}", app.api_url, conversation_id))
        .bearer_auth(&other)
        .send()
        .await
        .unwrap();
    assert_eq!(foreign.status(), StatusCode::NOT_FOUND);
    assert_eq!(messages(owner.clone()).await.len(), 1, "another user must not delete the messages");

    // The owner can still delete it
    let own = app
        .client
        .delete(format!("{}/conversations/{}", app.api_url, conversation_id))
        .bearer_auth(&owner)
        .send()
        .await
        .unwrap();
    assert_eq!(own.status(), StatusCode::NO_CONTENT);
}
