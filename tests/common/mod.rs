//! Shared test helpers. Not every test file uses all of them.
#![allow(dead_code)]

use reqwest::Client;
use serde_json::json;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};
use uuid::Uuid;

/// URL of the app under test. The first call starts the app in this test
/// process, on a free port and with a new SQLite database in the temp dir.
pub fn base_url() -> &'static str {
    static BASE_URL: OnceLock<String> = OnceLock::new();
    BASE_URL.get_or_init(start_app)
}

/// Run the app on its own thread and runtime: each #[tokio::test] has its own
/// runtime, which ends with the test, but the app must outlive every test.
fn start_app() -> String {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        runtime.block_on(async move {
            remove_old_test_databases();
            let db_path = std::env::temp_dir().join(format!("{DB_PREFIX}{}.db", Uuid::new_v4()));
            let config = mcp_client::config::Config {
                database_url: format!("sqlite://{}?mode=rwc", db_path.display()),
                host: std::net::IpAddr::from([127, 0, 0, 1]),
                port: 0,
                jwt_secret: "test-jwt-secret".to_string(),
                jwt_expiration_hours: 1,
                encryption_key: "test_encryption_key_32_bytes_xyz".to_string(),
            };
            let state = mcp_client::AppState {
                db: mcp_client::app::connect_db(&config.database_url).await.unwrap(),
                config,
                mcp_service: std::sync::Arc::new(mcp_client::services::mcp_client::McpClientService::new()),
                started_at: std::time::Instant::now(),
            };
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(format!("http://{}", listener.local_addr().unwrap())).unwrap();
            axum::serve(listener, mcp_client::app::router(state)).await.unwrap();
        });
    });
    rx.recv().expect("test app failed to start")
}

const DB_PREFIX: &str = "mcp-client-test-";

/// The app thread never ends cleanly, so delete databases of earlier runs.
fn remove_old_test_databases() {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else { return };
    let hour_ago = SystemTime::now() - Duration::from_secs(3600);
    for entry in entries.flatten() {
        let name = entry.file_name();
        let old = entry.metadata().and_then(|m| m.modified()).map(|t| t < hour_ago).unwrap_or(false);
        if name.to_string_lossy().starts_with(DB_PREFIX) && old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

pub struct TestApp {
    pub client: Client,
    pub api_url: String,
}

impl TestApp {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            api_url: format!("{}/api/v1", base_url()),
        }
    }

    pub async fn register_and_login(&self, username: &str, email: &str, password: &str) -> String {
        // Make username and email unique
        let unique_username = format!("{}_{}", username, Uuid::new_v4());
        let unique_email = format!("{}_{}", email, Uuid::new_v4());

        // Register
        let register_response = self
            .client
            .post(&format!("{}/auth/register", self.api_url))
            .json(&json!({
                "username": unique_username,
                "email": unique_email,
                "password": password
            }))
            .send()
            .await
            .expect("Failed to register");

        let body: serde_json::Value = register_response.json().await.expect("Failed to parse register response");
        body["token"].as_str().unwrap().to_string()
    }
}

pub async fn setup_test_app() -> TestApp {
    TestApp::new()
}
