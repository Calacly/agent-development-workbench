use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Authentication error: {0}")]
    Authentication(String),

    #[error("Authorization error: {0}")]
    Authorization(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("Internal server error: {0}")]
    Internal(String),

    #[error("MCP error: {0}")]
    Mcp(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("HTTP client error: {0}")]
    HttpClient(#[from] reqwest::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

impl AppError {
    /// HTTP status for this error (same mapping as the JSON response)
    pub fn into_response_status(&self) -> StatusCode {
        match self {
            AppError::Authentication(_) => StatusCode::UNAUTHORIZED,
            AppError::Authorization(_) => StatusCode::FORBIDDEN,
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::BadRequest(_) | AppError::Json(_) => StatusCode::BAD_REQUEST,
            AppError::Mcp(_) => StatusCode::BAD_GATEWAY,
            AppError::Database(_) | AppError::Internal(_) | AppError::Io(_) | AppError::HttpClient(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, error_message) = match self {
            AppError::Database(ref e) => {
                tracing::error!("Database error: {}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, "Database error")
            }
            AppError::Authentication(ref e) => {
                tracing::warn!("Authentication error: {}", e);
                (StatusCode::UNAUTHORIZED, "Authentication failed")
            }
            AppError::Authorization(ref e) => {
                tracing::warn!("Authorization error: {}", e);
                (StatusCode::FORBIDDEN, "Access denied")
            }
            AppError::NotFound(ref e) => {
                tracing::debug!("Not found: {}", e);
                (StatusCode::NOT_FOUND, "Resource not found")
            }
            AppError::BadRequest(ref e) => {
                tracing::warn!("Bad request: {}", e);
                (StatusCode::BAD_REQUEST, "Bad request")
            }
            AppError::Internal(ref e) => {
                tracing::error!("Internal server error: {}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
            }
            AppError::Mcp(ref e) => {
                tracing::error!("MCP server error: {}", e);
                (StatusCode::BAD_GATEWAY, "MCP server error")
            }
            AppError::Io(ref e) => {
                tracing::error!("IO error: {}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, "IO error")
            }
            AppError::HttpClient(ref e) => {
                tracing::error!("HTTP client error: {}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, "HTTP client error")
            }
            AppError::Json(ref e) => {
                tracing::error!("JSON error: {}", e);
                (StatusCode::BAD_REQUEST, "Invalid JSON")
            }
        };

        let body = Json(json!({
            "error": error_message,
            "details": self.to_string(),
        }));

        (status, body).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;
