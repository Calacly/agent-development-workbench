//! Sign in, register and sign out for the web UI. The session is the same JWT
//! the API issues, kept in an HttpOnly, SameSite=Strict cookie.

use askama::Template;
use axum::{
    extract::{Query, State},
    http::HeaderMap,
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Form, Json, Router,
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use serde::Deserialize;

use super::{render, session_user, user_message, Layout, WebResult};
use crate::{
    middleware::auth::SESSION_COOKIE,
    models::user::{CreateUserRequest, LoginRequest},
    routes::auth::{login, register},
    AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(home))
        .route("/login", get(login_page).post(login_submit))
        .route("/register", get(register_page).post(register_submit))
        .route("/logout", post(logout))
}

/// Only allow local paths as redirect target after login (no open redirect)
fn safe_next(next: &str) -> String {
    if next.starts_with('/') && !next.starts_with("//") && !next.contains('\\') {
        next.to_string()
    } else {
        "/dashboard".to_string()
    }
}

fn session_cookie(token: String, hours: i64) -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, token))
        .http_only(true)
        .same_site(SameSite::Strict)
        .path("/")
        .max_age(time::Duration::hours(hours))
        .build()
}

async fn home(State(state): State<AppState>, headers: HeaderMap) -> Redirect {
    if session_user(&state, &headers).is_some() {
        Redirect::to("/dashboard")
    } else {
        Redirect::to("/login")
    }
}

#[derive(Template)]
#[template(path = "login.html")]
struct LoginPage {
    layout: Layout,
    error: Option<String>,
    username: String,
    next: String,
}

#[derive(Deserialize)]
struct NextQuery {
    #[serde(default)]
    next: String,
}

async fn login_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<NextQuery>,
) -> WebResult<Response> {
    if session_user(&state, &headers).is_some() {
        return Ok(Redirect::to(&safe_next(&query.next)).into_response());
    }
    Ok(render(LoginPage { layout: Layout::anonymous(), error: None, username: String::new(), next: query.next })?
        .into_response())
}

#[derive(Deserialize)]
struct LoginForm {
    username: String,
    password: String,
    #[serde(default)]
    next: String,
}

async fn login_submit(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<LoginForm>,
) -> WebResult<Response> {
    let hours = state.config.jwt_expiration_hours;
    let request = LoginRequest { username: form.username.clone(), password: form.password };
    match login(State(state), Json(request)).await {
        Ok(Json(auth)) => Ok((jar.add(session_cookie(auth.token, hours)), Redirect::to(&safe_next(&form.next)))
            .into_response()),
        Err(_) => Ok(render(LoginPage {
            layout: Layout::anonymous(),
            error: Some("Wrong username or password.".to_string()),
            username: form.username,
            next: form.next,
        })?
        .into_response()),
    }
}

#[derive(Template)]
#[template(path = "register.html")]
struct RegisterPage {
    layout: Layout,
    error: Option<String>,
    username: String,
    email: String,
}

async fn register_page() -> WebResult<Html<String>> {
    render(RegisterPage { layout: Layout::anonymous(), error: None, username: String::new(), email: String::new() })
}

#[derive(Deserialize)]
struct RegisterForm {
    username: String,
    email: String,
    password: String,
}

async fn register_submit(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<RegisterForm>,
) -> WebResult<Response> {
    let hours = state.config.jwt_expiration_hours;
    let request = CreateUserRequest {
        username: form.username.trim().to_string(),
        email: form.email.trim().to_string(),
        password: form.password,
    };
    match register(State(state), Json(request)).await {
        Ok((_, Json(auth))) => {
            Ok((jar.add(session_cookie(auth.token, hours)), Redirect::to("/dashboard")).into_response())
        }
        Err(e) => Ok(render(RegisterPage {
            layout: Layout::anonymous(),
            error: Some(user_message(&e)),
            username: form.username,
            email: form.email,
        })?
        .into_response()),
    }
}

async fn logout(jar: CookieJar) -> (CookieJar, Redirect) {
    (jar.remove(Cookie::build(SESSION_COOKIE).path("/")), Redirect::to("/login"))
}

#[cfg(test)]
mod tests {
    use super::safe_next;

    #[test]
    fn next_must_be_a_local_path() {
        assert_eq!(safe_next("/llm-providers"), "/llm-providers");
        assert_eq!(safe_next("https://evil.example"), "/dashboard");
        assert_eq!(safe_next("//evil.example"), "/dashboard");
        assert_eq!(safe_next(""), "/dashboard");
    }
}
