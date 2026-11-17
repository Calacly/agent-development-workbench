//! Tests for the server-rendered web UI (pages, fragments, session cookie).

use reqwest::{header, redirect::Policy, Client, StatusCode};
use uuid::Uuid;

mod common;
use common::base_url;

fn client() -> Client {
    // Look at redirects instead of following them
    Client::builder().redirect(Policy::none()).build().unwrap()
}

/// Register through the web form and return the session cookie ("mcp_session=...")
async fn register(client: &Client) -> (String, String) {
    let username = format!("web_{}", Uuid::new_v4().simple());
    let response = client
        .post(format!("{}/register", base_url()))
        .form(&[("username", username.as_str()), ("email", &format!("{username}@example.com")), ("password", "testpass123")])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/dashboard");
    let set_cookie = response.headers()[header::SET_COOKIE].to_str().unwrap().to_string();
    assert!(set_cookie.starts_with("mcp_session="), "{set_cookie}");
    assert!(set_cookie.contains("HttpOnly"), "{set_cookie}");
    assert!(set_cookie.contains("SameSite=Strict"), "{set_cookie}");
    (set_cookie.split(';').next().unwrap().to_string(), username)
}

#[tokio::test]
async fn pages_redirect_to_login_without_a_session() {
    let client = client();
    let home = client.get(format!("{}/", base_url())).send().await.unwrap();
    assert_eq!(home.status(), StatusCode::SEE_OTHER);
    assert_eq!(home.headers()[header::LOCATION], "/login");

    let dashboard = client.get(format!("{}/dashboard", base_url())).send().await.unwrap();
    assert_eq!(dashboard.status(), StatusCode::SEE_OTHER);
    assert_eq!(dashboard.headers()[header::LOCATION], "/login?next=/dashboard");

    // htmx requests get HX-Redirect so the whole page navigates
    let fragment = client.post(format!("{}/llm-providers/test", base_url())).header("HX-Request", "true").send().await.unwrap();
    assert_eq!(fragment.headers()["HX-Redirect"], "/login");
}

#[tokio::test]
async fn login_page_shows_an_error_for_wrong_credentials() {
    let client = client();
    let page = client.get(format!("{}/login", base_url())).send().await.unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    assert!(page.text().await.unwrap().contains("<h1>Sign in</h1>"));

    let failed = client
        .post(format!("{}/login", base_url()))
        .form(&[("username", "nobody_here"), ("password", "wrongpass"), ("next", "")])
        .send()
        .await
        .unwrap();
    assert_eq!(failed.status(), StatusCode::OK);
    assert!(failed.headers().get(header::SET_COOKIE).is_none());
    assert!(failed.text().await.unwrap().contains("Wrong username or password."));
}

#[tokio::test]
async fn login_redirects_only_to_local_paths() {
    let client = client();
    let (_, username) = register(&client).await;
    let login = |next: &'static str| {
        client
            .post(format!("{}/login", base_url()))
            .form(&[("username", username.as_str()), ("password", "testpass123"), ("next", next)])
            .send()
    };
    assert_eq!(login("/llm-providers").await.unwrap().headers()[header::LOCATION], "/llm-providers");
    assert_eq!(login("https://evil.example").await.unwrap().headers()[header::LOCATION], "/dashboard");
}

#[tokio::test]
async fn signed_in_pages_render_with_the_session_cookie() {
    let client = client();
    let (cookie, username) = register(&client).await;

    let dashboard = client.get(format!("{}/dashboard", base_url())).header(header::COOKIE, &cookie).send().await.unwrap();
    assert_eq!(dashboard.status(), StatusCode::OK);
    let html = dashboard.text().await.unwrap();
    assert!(html.contains(&format!("Welcome back, {username}!")), "{html}");
    assert!(html.contains(r#"href="/dashboard" aria-current="page""#));

    let providers = client.get(format!("{}/llm-providers", base_url())).header(header::COOKIE, &cookie).send().await.unwrap();
    assert_eq!(providers.status(), StatusCode::OK);
    assert!(providers.text().await.unwrap().contains("No LLM providers yet"));

    // The JSON API accepts the same cookie (used by same-origin fetch and the chat WebSocket)
    let api = client.get(format!("{}/api/v1/llm-providers", base_url())).header(header::COOKIE, &cookie).send().await.unwrap();
    assert_eq!(api.status(), StatusCode::OK);
}

#[tokio::test]
async fn adding_a_provider_returns_the_updated_list() {
    let client = client();
    let (cookie, _) = register(&client).await;

    // Missing model: 422 fragment aimed at the dialog's error line
    let invalid = client
        .post(format!("{}/llm-providers", base_url()))
        .header(header::COOKIE, &cookie)
        .header("HX-Request", "true")
        .form(&[("name", "Local"), ("base_url", "http://localhost:11434/v1"), ("api_key", ""), ("model", "")])
        .send()
        .await
        .unwrap();
    assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(invalid.headers()["HX-Retarget"], "#add-provider-error");

    let created = client
        .post(format!("{}/llm-providers", base_url()))
        .header(header::COOKIE, &cookie)
        .header("HX-Request", "true")
        .form(&[("name", "Local <Ollama>"), ("base_url", "http://localhost:11434/v1"), ("api_key", ""), ("model", "qwen3")])
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let html = created.text().await.unwrap();
    assert!(html.starts_with(r#"<div id="provider-list">"#), "{html}");
    // Names are escaped, and the first provider becomes the default
    assert!(html.contains("Local &#60;Ollama&#62;") || html.contains("Local &lt;Ollama&gt;"), "{html}");
    assert!(html.contains("Default"));
}

#[tokio::test]
async fn logout_clears_the_cookie() {
    let client = client();
    let (cookie, _) = register(&client).await;
    let response = client.post(format!("{}/logout", base_url())).header(header::COOKIE, &cookie).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/login");
    let set_cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
    assert!(set_cookie.starts_with("mcp_session=;") || set_cookie.contains("Max-Age=0"), "{set_cookie}");
}

#[tokio::test]
async fn static_files_are_served_from_the_binary() {
    let client = client();
    let css = client.get(format!("{}/static/app.css", base_url())).send().await.unwrap();
    assert_eq!(css.status(), StatusCode::OK);
    assert_eq!(css.headers()[header::CONTENT_TYPE], "text/css");
    let htmx = client.get(format!("{}/static/vendor/htmx-2.0.11.min.js", base_url())).send().await.unwrap();
    assert_eq!(htmx.status(), StatusCode::OK);
    assert!(htmx.headers()[header::CACHE_CONTROL].to_str().unwrap().contains("immutable"));
    let missing = client.get(format!("{}/static/missing.js", base_url())).send().await.unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn chat_pages_create_redirect_and_stay_private() {
    let client = client();
    let (owner, _) = register(&client).await;
    let (other, _) = register(&client).await;

    let empty = client.get(format!("{}/chat", base_url())).header(header::COOKIE, &owner).send().await.unwrap();
    assert_eq!(empty.status(), StatusCode::OK);
    assert!(empty.text().await.unwrap().contains("Start a conversation"));

    // htmx gets a 200 with HX-Redirect (a 303 would be followed silently)
    let created = client
        .post(format!("{}/chat", base_url()))
        .header(header::COOKIE, &owner)
        .header("HX-Request", "true")
        .form(&[("title", "Plan")])
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let target = created.headers()["HX-Redirect"].to_str().unwrap().to_string();
    assert!(target.starts_with("/chat/"), "{target}");

    let home = client.get(format!("{}/chat", base_url())).header(header::COOKIE, &owner).send().await.unwrap();
    assert_eq!(home.status(), StatusCode::SEE_OTHER);
    assert_eq!(home.headers()[header::LOCATION].to_str().unwrap(), target);

    let page = client.get(format!("{}{target}", base_url())).header(header::COOKIE, &owner).send().await.unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    assert!(page.text().await.unwrap().contains("data-conversation-id"));

    // Another user can neither open the conversation nor read its messages
    let foreign = client.get(format!("{}{target}", base_url())).header(header::COOKIE, &other).send().await.unwrap();
    assert_eq!(foreign.status(), StatusCode::NOT_FOUND);
    let fragment = client.get(format!("{}{target}/messages/1", base_url())).header(header::COOKIE, &other).send().await.unwrap();
    assert_eq!(fragment.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn settings_choose_a_theme_that_the_next_page_uses() {
    let client = client();
    let (cookie, _) = register(&client).await;

    let page = client.get(format!("{}/settings", base_url())).header(header::COOKIE, &cookie).send().await.unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    let html = page.text().await.unwrap();
    assert_eq!(html.matches(r#"class="theme-card""#).count(), 40);

    let chosen = client
        .post(format!("{}/settings/theme", base_url()))
        .header(header::COOKIE, &cookie)
        .header("HX-Request", "true")
        .form(&[("theme_name", "Dracula")])
        .send()
        .await
        .unwrap();
    assert_eq!(chosen.headers()["HX-Refresh"], "true");

    let dashboard = client.get(format!("{}/dashboard", base_url())).header(header::COOKIE, &cookie).send().await.unwrap();
    let html = dashboard.text().await.unwrap();
    assert!(html.contains("--color-base-100:#282a36;"), "theme CSS missing");
    assert!(html.contains("color-scheme:dark;"));
}
