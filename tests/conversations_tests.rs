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
