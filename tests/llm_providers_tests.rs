use reqwest::StatusCode;
use serde_json::json;

mod common;
use common::setup_test_app;

#[tokio::test]
async fn test_create_llm_provider() {
    let app = setup_test_app().await;
    let token = app.register_and_login("testuser", "test@example.com", "testpass123").await;

    let response = app
        .client
        .post(&format!("{}/llm-providers", app.api_url))
        .bearer_auth(&token)
        .json(&json!({
            "name": "Test OpenAI",
            "base_url": "https://api.openai.com/v1",
            "api_key": "sk-test-key",
            "model": "gpt-4o",
            "is_default": true
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);

    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["name"], "Test OpenAI");
    assert_eq!(body["base_url"], "https://api.openai.com/v1");
    assert_eq!(body["model"], "gpt-4o");
    assert_eq!(body["is_default"], true);
    // The key is stored encrypted and never returned
    assert!(body.get("api_key_encrypted").is_none());
    assert_eq!(body["has_api_key"], true);
    assert!(body["id"].is_number());
}

#[tokio::test]
async fn test_list_llm_providers() {
    let app = setup_test_app().await;
    let token = app.register_and_login("testuser", "test@example.com", "testpass123").await;

    // Create two providers
    for i in 1..=2 {
        app.client
            .post(&format!("{}/llm-providers", app.api_url))
            .bearer_auth(&token)
            .json(&json!({
                "name": format!("Provider {}", i),
                "base_url": "https://api.example.com/v1",
                "api_key": "test-key",
                "model": "test-model",
                "is_default": i == 1
            }))
            .send()
            .await
            .unwrap();
    }

    let response = app
        .client
        .get(&format!("{}/llm-providers", app.api_url))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body: Vec<serde_json::Value> = response.json().await.unwrap();
    assert_eq!(body.len(), 2);
    assert_eq!(body[0]["name"], "Provider 1");
    assert_eq!(body[1]["name"], "Provider 2");
}

#[tokio::test]
async fn test_get_llm_provider() {
    let app = setup_test_app().await;
    let token = app.register_and_login("testuser", "test@example.com", "testpass123").await;

    // Create a provider
    let create_response = app
        .client
        .post(&format!("{}/llm-providers", app.api_url))
        .bearer_auth(&token)
        .json(&json!({
            "name": "Test Provider",
            "base_url": "https://api.example.com/v1",
            "api_key": "test-key",
            "model": "test-model",
            "is_default": false
        }))
        .send()
        .await
        .unwrap();

    let created: serde_json::Value = create_response.json().await.unwrap();
    let provider_id = created["id"].as_i64().unwrap();

    // Get the provider
    let response = app
        .client
        .get(&format!("{}/llm-providers/{}", app.api_url, provider_id))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["name"], "Test Provider");
    assert_eq!(body["id"], provider_id);
}

#[tokio::test]
async fn test_update_llm_provider() {
    let app = setup_test_app().await;
    let token = app.register_and_login("testuser", "test@example.com", "testpass123").await;

    // Create a provider
    let create_response = app
        .client
        .post(&format!("{}/llm-providers", app.api_url))
        .bearer_auth(&token)
        .json(&json!({
            "name": "Original Name",
            "base_url": "https://api.example.com/v1",
            "api_key": "test-key",
            "model": "test-model",
            "is_default": false
        }))
        .send()
        .await
        .unwrap();

    let created: serde_json::Value = create_response.json().await.unwrap();
    let provider_id = created["id"].as_i64().unwrap();

    // Update the provider
    let response = app
        .client
        .put(&format!("{}/llm-providers/{}", app.api_url, provider_id))
        .bearer_auth(&token)
        .json(&json!({
            "name": "Updated Name",
            "is_default": true
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["name"], "Updated Name");
    assert_eq!(body["is_default"], true);
}

#[tokio::test]
async fn test_delete_llm_provider() {
    let app = setup_test_app().await;
    let token = app.register_and_login("testuser", "test@example.com", "testpass123").await;

    // Create a provider
    let create_response = app
        .client
        .post(&format!("{}/llm-providers", app.api_url))
        .bearer_auth(&token)
        .json(&json!({
            "name": "To Delete",
            "base_url": "https://api.example.com/v1",
            "api_key": "test-key",
            "model": "test-model",
            "is_default": false
        }))
        .send()
        .await
        .unwrap();

    let created: serde_json::Value = create_response.json().await.unwrap();
    let provider_id = created["id"].as_i64().unwrap();

    // Delete the provider
    let response = app
        .client
        .delete(&format!("{}/llm-providers/{}", app.api_url, provider_id))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Verify it's deleted
    let get_response = app
        .client
        .get(&format!("{}/llm-providers/{}", app.api_url, provider_id))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();

    assert_eq!(get_response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_set_default_provider() {
    let app = setup_test_app().await;
    let token = app.register_and_login("testuser", "test@example.com", "testpass123").await;

    // Create two providers
    let provider1 = app
        .client
        .post(&format!("{}/llm-providers", app.api_url))
        .bearer_auth(&token)
        .json(&json!({
            "name": "Provider 1",
            "base_url": "https://api.example.com/v1",
            "api_key": "test-key-1",
            "model": "test-model",
            "is_default": true
        }))
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();

    let provider2 = app
        .client
        .post(&format!("{}/llm-providers", app.api_url))
        .bearer_auth(&token)
        .json(&json!({
            "name": "Provider 2",
            "base_url": "https://api.example.com/v1",
            "api_key": "test-key-2",
            "model": "test-model",
            "is_default": false
        }))
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();

    let provider2_id = provider2["id"].as_i64().unwrap();

    // Set provider 2 as default
    app.client
        .put(&format!("{}/llm-providers/{}", app.api_url, provider2_id))
        .bearer_auth(&token)
        .json(&json!({
            "is_default": true
        }))
        .send()
        .await
        .unwrap();

    // Verify provider 1 is no longer default
    let provider1_updated = app
        .client
        .get(&format!("{}/llm-providers/{}", app.api_url, provider1["id"].as_i64().unwrap()))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();

    assert_eq!(provider1_updated["is_default"], false);

    // Verify provider 2 is default
    let provider2_updated = app
        .client
        .get(&format!("{}/llm-providers/{}", app.api_url, provider2_id))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();

    assert_eq!(provider2_updated["is_default"], true);
}

#[tokio::test]
async fn test_provider_isolation_between_users() {
    let app = setup_test_app().await;
    let token1 = app.register_and_login("user1", "user1@example.com", "password123").await;
    let token2 = app.register_and_login("user2", "user2@example.com", "password123").await;

    // User 1 creates a provider
    let provider = app
        .client
        .post(&format!("{}/llm-providers", app.api_url))
        .bearer_auth(&token1)
        .json(&json!({
            "name": "User 1 Provider",
            "base_url": "https://api.example.com/v1",
            "api_key": "user1-key",
            "model": "test-model",
            "is_default": false
        }))
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();

    let provider_id = provider["id"].as_i64().unwrap();

    // User 2 tries to access user 1's provider
    let response = app
        .client
        .get(&format!("{}/llm-providers/{}", app.api_url, provider_id))
        .bearer_auth(&token2)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // User 2's list should be empty
    let list_response = app
        .client
        .get(&format!("{}/llm-providers", app.api_url))
        .bearer_auth(&token2)
        .send()
        .await
        .unwrap()
        .json::<Vec<serde_json::Value>>()
        .await
        .unwrap();

    assert_eq!(list_response.len(), 0);
}

#[tokio::test]
async fn test_unauthorized_access() {
    let app = setup_test_app().await;

    let response = app
        .client
        .get(&format!("{}/llm-providers", app.api_url))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
