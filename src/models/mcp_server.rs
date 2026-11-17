use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpServerType {
    Stdio,
    WebSocket,
    Http,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpServerStatus {
    Stopped,
    Starting,
    Running,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct McpServer {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub server_type: String, // stored as string in DB
    pub config: String,      // JSON config
    pub status: String,      // stored as string in DB
    pub use_container: bool, // Run in container (Podman/Docker)
    pub container_image: Option<String>, // Custom container image
    pub container_params: Option<String>, // Additional container parameters (e.g., "-v /data:/data")
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateMcpServerRequest {
    pub name: String,
    pub server_type: McpServerType,
    pub config: serde_json::Value,
    #[serde(default)]
    pub use_container: bool,
    pub container_image: Option<String>,
    pub container_params: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateMcpServerRequest {
    pub name: Option<String>,
    pub config: Option<serde_json::Value>,
    pub use_container: Option<bool>,
    pub container_image: Option<String>,
    pub container_params: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct McpServerResponse {
    pub id: i64,
    pub name: String,
    pub server_type: McpServerType,
    pub config: serde_json::Value,
    pub status: McpServerStatus,
    pub use_container: bool,
    pub container_image: Option<String>,
    pub container_params: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
