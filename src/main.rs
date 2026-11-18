use mcp_client::{config::Config, AppState};
use std::net::SocketAddr;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Create logs directory if it doesn't exist
    std::fs::create_dir_all("./logs")?;

    // Set up file appender for MCP connection logs
    let file_appender = tracing_appender::rolling::daily("./logs", "mcp_connections.log");
    let (non_blocking, _file_guard) = tracing_appender::non_blocking(file_appender);
    // Note: _file_guard must be kept in scope for the entire program lifetime

    // Initialize tracing with both console and file output
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "mcp_client=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer()) // Console output
        .with(tracing_subscriber::fmt::layer()
            .with_writer(non_blocking)
            .with_ansi(false)) // File output without color codes
        .init();

    // Load configuration
    let config = Config::from_env()?;

    let db_pool = mcp_client::app::connect_db(&config.database_url).await?;

    // Initialize MCP client service
    let mcp_service = std::sync::Arc::new(mcp_client::services::mcp_client::McpClientService::new());

    // Build application state
    let state = AppState {
        db: db_pool,
        config: config.clone(),
        mcp_service,
        started_at: std::time::Instant::now(),
    };

    mcp_client::app::reconnect_running_servers(&state).await?;

    let app = mcp_client::app::router(state);

    // Run server
    let addr = SocketAddr::new(config.host, config.port);
    tracing::info!("Starting server on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
