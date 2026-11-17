//! Server-rendered web UI: askama templates (backend/templates), htmx and a
//! little plain JavaScript (backend/static). Everything is part of the binary,
//! so `cargo run` serves the whole app. In debug builds rust-embed reads the
//! static files from disk, so CSS/JS edits show up on reload.

mod auth;
mod chat;
mod dashboard;
mod mcp_import;
mod providers;
mod servers;
mod settings;
pub mod theme;
pub mod view;

use askama::Template;
use axum::{
    extract::{Path, Request, State},
    http::{header, HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Redirect, Response},
    routing::get,
    Router,
};
use rust_embed::RustEmbed;

use crate::{
    error::AppError,
    middleware::auth::{session_cookie, AuthUser},
    models::user_settings::{CustomTheme, UserSettings},
    services::auth::AuthService,
    AppState,
};

#[derive(RustEmbed)]
#[folder = "static/"]
struct Assets;

pub fn routes(state: AppState) -> Router<AppState> {
    let protected = Router::new()
        .route("/dashboard", get(dashboard::page))
        .merge(providers::routes())
        .merge(servers::routes())
        .merge(chat::routes())
        .merge(settings::routes())
        .layer(middleware::from_fn_with_state(state, require_web_auth));

    Router::new()
        .merge(auth::routes())
        .route("/static/*path", get(static_file))
        .merge(protected)
}

/// Serve an embedded static file
async fn static_file(Path(path): Path<String>) -> Response {
    match Assets::get(&path) {
        Some(file) => {
            let mime = mime_guess::from_path(&path).first_or_octet_stream();
            (
                [
                    (header::CONTENT_TYPE, mime.as_ref().to_string()),
                    // Vendored files carry their version in the name; app files are revalidated
                    (
                        header::CACHE_CONTROL,
                        if path.starts_with("vendor/") { "public, max-age=31536000, immutable" } else { "no-cache" }
                            .to_string(),
                    ),
                ],
                file.data.into_owned(),
            )
                .into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

// ---------- Errors ----------

/// Error page for the web UI (the JSON API keeps AppError's JSON body)
pub struct WebError(pub AppError);

impl From<AppError> for WebError {
    fn from(e: AppError) -> Self {
        WebError(e)
    }
}

impl From<askama::Error> for WebError {
    fn from(e: askama::Error) -> Self {
        WebError(AppError::Internal(format!("Template error: {e}")))
    }
}

impl From<sqlx::Error> for WebError {
    fn from(e: sqlx::Error) -> Self {
        WebError(AppError::Database(e))
    }
}

#[derive(Template)]
#[template(path = "error.html")]
struct ErrorPage {
    layout: Layout,
    status: u16,
    message: String,
}

impl IntoResponse for WebError {
    fn into_response(self) -> Response {
        let status = self.0.into_response_status();
        let message = user_message(&self.0);
        let page = ErrorPage { layout: Layout::anonymous(), status: status.as_u16(), message };
        match page.render() {
            Ok(html) => (status, Html(html)).into_response(),
            Err(_) => (status, "Something went wrong").into_response(),
        }
    }
}

/// Message for the user, without internal details for server errors
pub fn user_message(e: &AppError) -> String {
    match e {
        AppError::BadRequest(m) | AppError::NotFound(m) => m.clone(),
        AppError::Authentication(_) => "Please sign in again.".to_string(),
        AppError::Authorization(_) => "You are not allowed to do this.".to_string(),
        AppError::Mcp(m) => m.clone(),
        AppError::Internal(m) => m.clone(),
        _ => "Something went wrong. Please try again.".to_string(),
    }
}

pub type WebResult<T> = Result<T, WebError>;

/// Render a template into an HTML response
pub fn render<T: Template>(template: T) -> WebResult<Html<String>> {
    Ok(Html(template.render()?))
}

// ---------- Layout ----------

pub struct NavItem {
    pub href: &'static str,
    pub label: &'static str,
    pub active: bool,
}

/// Data every page needs: who is signed in, the theme, the navigation
pub struct Layout {
    pub username: String,
    pub theme_css: String,
    pub nav: Vec<NavItem>,
}

impl Layout {
    /// Layout for pages without a signed-in user (default theme)
    pub fn anonymous() -> Self {
        Layout { username: String::new(), theme_css: String::new(), nav: Vec::new() }
    }

    pub async fn for_user(state: &AppState, user: &AuthUser, active: &str) -> WebResult<Self> {
        let settings = sqlx::query_as::<_, UserSettings>("SELECT * FROM user_settings WHERE user_id = ?")
            .bind(user.user_id)
            .fetch_optional(&state.db)
            .await?;
        let theme_css = settings
            .and_then(|s| {
                let custom: Vec<CustomTheme> =
                    s.custom_themes.as_deref().and_then(|j| serde_json::from_str(j).ok()).unwrap_or_default();
                theme::find_theme(&s.theme_name, &custom)
            })
            .map(|t| theme::theme_css(&t))
            .unwrap_or_default();
        let nav = [
            ("/dashboard", "Dashboard"),
            ("/llm-providers", "LLM Providers"),
            ("/mcp-servers", "MCP Servers"),
            ("/chat", "Chat"),
        ]
        .into_iter()
        .map(|(href, label)| NavItem { href, label, active: href == active })
        .collect();
        Ok(Layout { username: user.username.clone(), theme_css, nav })
    }
}

// ---------- Auth ----------

/// Signed-in user from the session cookie, if the token is valid
pub fn session_user(state: &AppState, headers: &HeaderMap) -> Option<AuthUser> {
    let token = session_cookie(headers)?;
    let claims = AuthService::new(state.config.jwt_secret.clone(), state.config.jwt_expiration_hours)
        .validate_token(&token)
        .ok()?;
    Some(AuthUser { user_id: claims.sub.parse().ok()?, username: claims.username })
}

/// Pages need a session; without one, go to the login page (htmx requests
/// get an HX-Redirect so the whole page navigates, not just a fragment).
async fn require_web_auth(State(state): State<AppState>, mut request: Request, next: Next) -> Response {
    match session_user(&state, request.headers()) {
        Some(user) => {
            request.extensions_mut().insert(user);
            next.run(request).await
        }
        None if request.headers().contains_key("hx-request") => {
            ([("HX-Redirect", "/login")], StatusCode::OK).into_response()
        }
        None => {
            let next_path = request.uri().path().to_string();
            Redirect::to(&format!("/login?next={}", urlencode(&next_path))).into_response()
        }
    }
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'/' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Thousands separator for token counts ("8,000")
pub fn format_number(n: i64) -> String {
    let digits = n.abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 {
        format!("-{out}")
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_numbers_with_separators() {
        assert_eq!(format_number(8000), "8,000");
        assert_eq!(format_number(1234567), "1,234,567");
        assert_eq!(format_number(12), "12");
    }

    #[test]
    fn urlencodes_paths() {
        assert_eq!(urlencode("/llm-providers"), "/llm-providers");
        assert_eq!(urlencode("/a b?c"), "/a%20b%3Fc");
    }
}
