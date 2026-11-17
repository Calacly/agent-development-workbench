use axum::{extract::State, http::StatusCode, response::IntoResponse};
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use std::time::Instant;

/// Metrics recorder handle
pub struct MetricsRecorder {
    handle: PrometheusHandle,
}

impl MetricsRecorder {
    pub fn new() -> Self {
        let handle = PrometheusBuilder::new()
            .set_buckets_for_metric(
                Matcher::Full("http_request_duration_seconds".to_string()),
                &[0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0],
            )
            .unwrap()
            .install_recorder()
            .unwrap();

        Self { handle }
    }

    pub fn handle(&self) -> PrometheusHandle {
        self.handle.clone()
    }
}

/// Handler for Prometheus metrics endpoint
pub async fn metrics_handler(State(state): State<crate::AppState>) -> impl IntoResponse {
    let metrics = state.metrics_handle.render();
    (StatusCode::OK, metrics)
}

/// Record HTTP request metrics
pub fn record_http_request(method: &str, path: &str, status: u16, duration_secs: f64) {
    metrics::increment_counter!("http_requests_total", "method" => method.to_string(), "path" => path.to_string(), "status" => status.to_string());
    metrics::histogram!("http_request_duration_seconds", duration_secs, "method" => method.to_string(), "path" => path.to_string());
}

/// Record database operation metrics
pub fn record_db_operation(operation: &str, table: &str, duration_secs: f64) {
    metrics::increment_counter!("db_operations_total", "operation" => operation.to_string(), "table" => table.to_string());
    metrics::histogram!("db_operation_duration_seconds", duration_secs, "operation" => operation.to_string(), "table" => table.to_string());
}

/// Record MCP operation metrics
pub fn record_mcp_operation(operation: &str, server_id: i64, success: bool, duration_secs: f64) {
    metrics::increment_counter!("mcp_operations_total", "operation" => operation.to_string(), "server_id" => server_id.to_string(), "success" => success.to_string());
    metrics::histogram!("mcp_operation_duration_seconds", duration_secs, "operation" => operation.to_string());
}

/// Record LLM operation metrics
pub fn record_llm_operation(provider: &str, model: &str, success: bool, duration_secs: f64, tokens: Option<u64>) {
    metrics::increment_counter!("llm_requests_total", "provider" => provider.to_string(), "model" => model.to_string(), "success" => success.to_string());
    metrics::histogram!("llm_request_duration_seconds", duration_secs, "provider" => provider.to_string(), "model" => model.to_string());

    if let Some(token_count) = tokens {
        // Track token count as a histogram since counter! macro doesn't support custom increments with labels
        metrics::histogram!("llm_tokens_total", token_count as f64, "provider" => provider.to_string(), "model" => model.to_string());
    }
}

/// Record WebSocket connection metrics
pub fn record_websocket_connection(action: &str) {
    metrics::increment_counter!("websocket_connections_total", "action" => action.to_string());
}

/// Record active connections gauge
pub fn record_active_connections(count: i64) {
    metrics::gauge!("active_connections", count as f64);
}

/// Record active MCP servers gauge
pub fn record_active_mcp_servers(count: i64) {
    metrics::gauge!("active_mcp_servers", count as f64);
}

/// Timer helper for tracking operation duration
pub struct MetricsTimer {
    start: Instant,
}

impl MetricsTimer {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }

    pub fn elapsed_secs(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }
}
