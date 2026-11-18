//! Building blocks of the server, shared by `main` and the integration tests.

use crate::{routes, AppState};
use axum::{extract::State, routing::get, Router};
use sqlx::SqlitePool;
use tower_http::cors::CorsLayer;

/// Connect to the database and run the migrations.
pub async fn connect_db(database_url: &str) -> anyhow::Result<SqlitePool> {
    let pool = crate::db::init_db(database_url).await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
}

/// The whole app: web UI, health and the JSON API.
pub fn router(state: AppState) -> Router {
    Router::new()
        // Server-rendered web UI (pages, fragments, static files)
        .merge(crate::web::routes(state.clone()))
        .route("/health", get(health))
        .nest("/api/v1", routes::api_routes(state.clone()))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

/// Reconnect the MCP servers that were running when the app stopped.
pub async fn reconnect_running_servers(state: &AppState) -> anyhow::Result<()> {
    tracing::info!("Auto-reconnecting to running MCP servers...");
    let running_servers = sqlx::query_as::<_, crate::models::mcp_server::McpServer>(
        "SELECT * FROM mcp_servers WHERE status = 'running'"
    )
    .fetch_all(&state.db)
    .await?;

    for server in running_servers {
        tracing::info!("Reconnecting to MCP server: {} (id: {})", server.name, server.id);

        // Parse config
        let config_result: Result<serde_json::Value, _> = serde_json::from_str(&server.config);
        let server_config = match config_result {
            Ok(cfg) => cfg,
            Err(e) => {
                tracing::error!("Failed to parse config for server {}: {}", server.name, e);
                continue;
            }
        };

        // Create transport based on server type
        let transport = match server.server_type.as_str() {
            "stdio" => {
                let command = server_config["command"]
                    .as_str()
                    .unwrap_or("npx")
                    .to_string();
                let args = server_config["args"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                let env = server_config["env"].as_object().map(|obj| {
                    obj.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                });

                crate::services::mcp_client::McpTransport::Stdio { command, args, env }
            }
            "http" => {
                let url = server_config["url"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                let bearer_token = server_config["bearer_token"]
                    .as_str()
                    .map(|s| s.to_string());
                crate::services::mcp_client::McpTransport::Http { url, bearer_token }
            }
            _ => {
                tracing::warn!("Unsupported server type '{}' for server {}", server.server_type, server.name);
                continue;
            }
        };

        // Attempt to connect with default sandbox configuration
        match state.mcp_service.connect_server(
            server.id,
            server.name.clone(),
            transport,
            None, // sandbox config
            server.use_container,
            server.container_image.clone(),
            server.container_params.clone(),
        ).await {
            Ok(_) => {
                tracing::info!("Successfully reconnected to server: {}", server.name);
            }
            Err(e) => {
                tracing::error!("Failed to reconnect to server {}: {}", server.name, e);
                // Update status to stopped in database
                let _ = sqlx::query("UPDATE mcp_servers SET status = 'stopped' WHERE id = ?")
                    .bind(server.id)
                    .execute(&state.db)
                    .await;
            }
        }
    }

    Ok(())
}

async fn health(State(state): State<AppState>) -> axum::Json<serde_json::Value> {
    // Check database connection
    let db_healthy = sqlx::query("SELECT 1")
        .fetch_one(&state.db)
        .await
        .is_ok();

    // Count active MCP servers
    let active_servers = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM mcp_servers WHERE status = 'running'"
    )
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    // Overall health status
    let status = if db_healthy { "healthy" } else { "unhealthy" };

    axum::Json(serde_json::json!({
        "status": status,
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_seconds": state.started_at.elapsed().as_secs(),
        "checks": {
            "database": if db_healthy { "ok" } else { "error" },
            "active_mcp_servers": active_servers
        }
    }))
}
