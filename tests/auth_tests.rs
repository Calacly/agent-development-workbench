use reqwest::{Client, StatusCode};
use serde_json::json;

mod common;
use common::base_url;

#[tokio::test]
async fn test_user_registration() {
    let client = Client::new();

    let response = client
        .post(format!("{}/api/v1/auth/register", base_url()))
        .json(&json!({
            "username": format!("user_{}", uuid::Uuid::new_v4()),
            "email": format!("test_{}@example.com", uuid::Uuid::new_v4()),
            "password": "password123"
        }))
        .send()
        .await
        .expect("Failed to send request");

    assert_eq!(response.status(), StatusCode::CREATED);

    let body: serde_json::Value = response.json().await.expect("Failed to parse response");
    assert!(body.get("token").is_some());
    assert!(body.get("user").is_some());
}

#[tokio::test]
async fn test_registration_duplicate_username() {
    let client = Client::new();
    let username = format!("duplicate_{}", uuid::Uuid::new_v4());
    let email1 = format!("test1_{}@example.com", uuid::Uuid::new_v4());
    let email2 = format!("test2_{}@example.com", uuid::Uuid::new_v4());

    // First registration
    client
        .post(format!("{}/api/v1/auth/register", base_url()))
        .json(&json!({
            "username": username,
            "email": email1,
            "password": "password123"
        }))
        .send()
        .await
        .expect("Failed to send request");

    // Second registration with same username
    let response = client
        .post(format!("{}/api/v1/auth/register", base_url()))
        .json(&json!({
            "username": username,
            "email": email2,
            "password": "password123"
        }))
        .send()
        .await
        .expect("Failed to send request");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_registration_invalid_password() {
    let client = Client::new();

    let response = client
        .post(format!("{}/api/v1/auth/register", base_url()))
        .json(&json!({
            "username": format!("user_{}", uuid::Uuid::new_v4()),
            "email": format!("test_{}@example.com", uuid::Uuid::new_v4()),
            "password": "short"
        }))
        .send()
        .await
        .expect("Failed to send request");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_user_login() {
    let client = Client::new();
    let username = format!("loginuser_{}", uuid::Uuid::new_v4());
    let password = "password123";

    // Register user
    client
        .post(format!("{}/api/v1/auth/register", base_url()))
        .json(&json!({
            "username": username,
            "email": format!("{}@example.com", username),
            "password": password
        }))
        .send()
        .await
        .expect("Failed to register");

    // Login
    let response = client
        .post(format!("{}/api/v1/auth/login", base_url()))
        .json(&json!({
            "username": username,
            "password": password
        }))
        .send()
        .await
        .expect("Failed to login");

    assert_eq!(response.status(), StatusCode::OK);

    let body: serde_json::Value = response.json().await.expect("Failed to parse response");
    assert!(body.get("token").is_some());
    assert!(body.get("user").is_some());
}

#[tokio::test]
async fn test_login_invalid_credentials() {
    let client = Client::new();

    let response = client
        .post(format!("{}/api/v1/auth/login", base_url()))
        .json(&json!({
            "username": "nonexistent",
            "password": "wrongpassword"
        }))
        .send()
        .await
        .expect("Failed to send request");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_login_wrong_password() {
    let client = Client::new();
    let username = format!("wrongpass_{}", uuid::Uuid::new_v4());

    // Register user
    client
        .post(format!("{}/api/v1/auth/register", base_url()))
        .json(&json!({
            "username": username,
            "email": format!("{}@example.com", username),
            "password": "correctpassword"
        }))
        .send()
        .await
        .expect("Failed to register");

    // Try login with wrong password
    let response = client
        .post(format!("{}/api/v1/auth/login", base_url()))
        .json(&json!({
            "username": username,
            "password": "wrongpassword"
        }))
        .send()
        .await
        .expect("Failed to send request");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
