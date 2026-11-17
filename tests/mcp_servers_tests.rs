use reqwest::StatusCode;
use serde_json::json;

mod common;
use common::setup_test_app;

#[tokio::test]
async fn test_create_stdio_mcp_server() {
    let app = setup_test_app().await;
    let token = app.register_and_login("testuser", "test@example.com", "testpass123").await;

    let response = app
        .client
        .post(&format!("{}/mcp-servers", app.api_url))
        .bearer_auth(&token)
        .json(&json!({
            "name": "Git Server",
            "server_type": "stdio",
            "config": {
                "command": "uvx",
                "args": ["mcp-server-git"],
                "env": {}
            }
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);

    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["name"], "Git Server");
    assert_eq!(body["server_type"], "stdio");
    assert_eq!(body["status"], "stopped");
    assert!(body["id"].is_number());
}

#[tokio::test]
async fn test_create_http_mcp_server() {
    let app = setup_test_app().await;
    let token = app.register_and_login("testuser", "test@example.com", "testpass123").await;

    let response = app
        .client
        .post(&format!("{}/mcp-servers", app.api_url))
        .bearer_auth(&token)
        .json(&json!({
            "name": "Remote Server",
            "server_type": "http",
            "config": {
                "url": "https://mcp.example.com"
            }
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);

    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["name"], "Remote Server");
    assert_eq!(body["server_type"], "http");
}

#[tokio::test]
async fn test_list_mcp_servers() {
    let app = setup_test_app().await;
    let token = app.register_and_login("testuser", "test@example.com", "testpass123").await;

    // Create two servers
    for i in 1..=2 {
        app.client
            .post(&format!("{}/mcp-servers", app.api_url))
            .bearer_auth(&token)
            .json(&json!({
                "name": format!("Server {}", i),
                "server_type": "stdio",
                "config": {
                    "command": "node",
                    "args": ["server.js"],
                    "env": {}
                }
            }))
            .send()
            .await
            .unwrap();
    }

    let response = app
        .client
        .get(&format!("{}/mcp-servers", app.api_url))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body: Vec<serde_json::Value> = response.json().await.unwrap();
    assert!(body.len() >= 2);
}

#[tokio::test]
async fn test_get_mcp_server() {
    let app = setup_test_app().await;
    let token = app.register_and_login("testuser", "test@example.com", "testpass123").await;

    // Create a server
    let create_response = app
        .client
        .post(&format!("{}/mcp-servers", app.api_url))
        .bearer_auth(&token)
        .json(&json!({
            "name": "Test Server",
            "server_type": "stdio",
            "config": {
                "command": "node",
                "args": ["server.js"],
                "env": {}
            }
        }))
        .send()
        .await
        .unwrap();

    let created: serde_json::Value = create_response.json().await.unwrap();
    let server_id = created["id"].as_i64().unwrap();

    // Get the server
    let response = app
        .client
        .get(&format!("{}/mcp-servers/{}", app.api_url, server_id))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["name"], "Test Server");
    assert_eq!(body["id"], server_id);
}

#[tokio::test]
async fn test_update_mcp_server() {
    let app = setup_test_app().await;
    let token = app.register_and_login("testuser", "test@example.com", "testpass123").await;

    // Create a server
    let create_response = app
        .client
        .post(&format!("{}/mcp-servers", app.api_url))
        .bearer_auth(&token)
        .json(&json!({
            "name": "Original Name",
            "server_type": "stdio",
            "config": {
                "command": "node",
                "args": ["server.js"],
                "env": {}
            }
        }))
        .send()
        .await
        .unwrap();

    let created: serde_json::Value = create_response.json().await.unwrap();
    let server_id = created["id"].as_i64().unwrap();

    // Update the server
    let response = app
        .client
        .put(&format!("{}/mcp-servers/{}", app.api_url, server_id))
        .bearer_auth(&token)
        .json(&json!({
            "name": "Updated Name"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["name"], "Updated Name");
}

#[tokio::test]
async fn test_delete_mcp_server() {
    let app = setup_test_app().await;
    let token = app.register_and_login("testuser", "test@example.com", "testpass123").await;

    // Create a server
    let create_response = app
        .client
        .post(&format!("{}/mcp-servers", app.api_url))
        .bearer_auth(&token)
        .json(&json!({
            "name": "To Delete",
            "server_type": "stdio",
            "config": {
                "command": "node",
                "args": ["server.js"],
                "env": {}
            }
        }))
        .send()
        .await
        .unwrap();

    let created: serde_json::Value = create_response.json().await.unwrap();
    let server_id = created["id"].as_i64().unwrap();

    // Delete the server
    let response = app
        .client
        .delete(&format!("{}/mcp-servers/{}", app.api_url, server_id))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Verify it's deleted
    let get_response = app
        .client
        .get(&format!("{}/mcp-servers/{}", app.api_url, server_id))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();

    assert_eq!(get_response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_server_isolation_between_users() {
    let app = setup_test_app().await;
    let token1 = app.register_and_login("user1", "user1@example.com", "password123").await;
    let token2 = app.register_and_login("user2", "user2@example.com", "password123").await;

    // User 1 creates a server
    let server = app
        .client
        .post(&format!("{}/mcp-servers", app.api_url))
        .bearer_auth(&token1)
        .json(&json!({
            "name": "User 1 Server",
            "server_type": "stdio",
            "config": {
                "command": "node",
                "args": ["server.js"],
                "env": {}
            }
        }))
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();

    let server_id = server["id"].as_i64().unwrap();

    // User 2 tries to access user 1's server
    let response = app
        .client
        .get(&format!("{}/mcp-servers/{}", app.api_url, server_id))
        .bearer_auth(&token2)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_unauthorized_access_to_mcp_servers() {
    let app = setup_test_app().await;

    let response = app
        .client
        .get(&format!("{}/mcp-servers", app.api_url))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
