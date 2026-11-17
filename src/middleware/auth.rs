use axum::{
    async_trait,
    extract::{FromRequestParts, Request, State},
    http::{header, request::Parts},
    middleware::Next,
    response::Response,
};

use crate::{error::AppError, services::auth::AuthService, AppState};

/// Extension type to add authenticated user info to requests
#[derive(Clone)]
pub struct AuthUser {
    pub user_id: i64,
    pub username: String,
}

#[async_trait]
impl<S> FromRequestParts<S> for AuthUser
where
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<AuthUser>()
            .cloned()
            .ok_or_else(|| AppError::Authentication("User not authenticated".to_string()))
    }
}

/// Name of the HttpOnly cookie that holds the session JWT for the web UI
pub const SESSION_COOKIE: &str = "mcp_session";

/// Session JWT from the Cookie header, if present
pub fn session_cookie(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == SESSION_COOKIE)
        .map(|(_, value)| value.to_string())
}

/// Middleware to require authentication
pub async fn require_auth(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, AppError> {
    // Authorization header first, then the browser session cookie (server-rendered
    // pages, same-origin fetch and WebSocket), then the ?token= query parameter
    let cookie_token = session_cookie(request.headers());
    let token = if let Some(auth_header) = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
    {
        // Extract token from "Bearer <token>" format
        auth_header
            .strip_prefix("Bearer ")
            .ok_or_else(|| AppError::Authentication("Invalid authorization format".to_string()))?
    } else if let Some(token) = cookie_token.as_deref() {
        token
    } else {
        // Try to get token from query parameter (for WebSocket connections)
        request
            .uri()
            .query()
            .and_then(|q| {
                q.split('&')
                    .find(|pair| pair.starts_with("token="))
                    .and_then(|pair| pair.strip_prefix("token="))
            })
            .ok_or_else(|| AppError::Authentication("Missing authorization token".to_string()))?
    };

    let auth_service = AuthService::new(
        state.config.jwt_secret.clone(),
        state.config.jwt_expiration_hours,
    );

    let claims = auth_service.validate_token(token)?;

    let user_id: i64 = claims
        .sub
        .parse()
        .map_err(|_| AppError::Authentication("Invalid user ID in token".to_string()))?;

    // Add user info to request extensions
    request.extensions_mut().insert(AuthUser {
        user_id,
        username: claims.username,
    });

    Ok(next.run(request).await)
}
