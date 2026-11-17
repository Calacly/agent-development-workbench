use axum::{middleware, routing::get, Router};

pub mod auth;
pub mod llm_providers;
pub mod mcp_servers;
pub mod conversations;
pub mod ws_chat;
pub mod settings;

use crate::{middleware::auth::require_auth, AppState};

pub fn api_routes(state: AppState) -> Router<AppState> {
    let protected = Router::new()
        .nest("/llm-providers", llm_providers::routes())
        .nest("/mcp-servers", mcp_servers::routes())
        .nest("/conversations", conversations::routes())
        .nest("/settings", settings::routes())
        .route("/ws/conversations/:id", get(ws_chat::ws_chat_handler))
        .nest("/auth", auth::protected_routes())
        .layer(middleware::from_fn_with_state(state.clone(), require_auth));

    Router::new()
        .nest("/auth", auth::routes())
        .merge(protected)
}
