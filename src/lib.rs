pub mod app;
pub mod config;
pub mod container;
pub mod db;
pub mod error;
pub mod metrics;
pub mod middleware;
pub mod models;
pub mod routes;
pub mod sandbox;
pub mod services;
pub mod web;

use metrics_exporter_prometheus::PrometheusHandle;
use services::mcp_client::McpClientService;
use std::sync::Arc;

// Re-export AppState for convenience
#[derive(Clone)]
pub struct AppState {
    pub db: sqlx::SqlitePool,
    pub config: config::Config,
    pub mcp_service: Arc<McpClientService>,
    pub metrics_handle: PrometheusHandle,
    /// When the server started, for the uptime in /health
    pub started_at: std::time::Instant,
}
