//! Settings page: choose one of the built-in or custom themes, generate a
//! theme with the default LLM, delete custom themes. A change reloads the
//! page so the server renders it with the new theme.

use askama::Template;
use axum::{
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Extension, Form, Json, Router,
};
use serde::Deserialize;

use super::{render, theme, user_message, Layout, WebResult};
use crate::{
    middleware::auth::AuthUser,
    models::user_settings::{DeleteThemeRequest, GenerateThemeRequest, UpdateSettingsRequest},
    routes::{
        llm_providers::list_providers,
        settings::{delete_theme, generate_theme, get_settings, update_settings},
    },
    AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/settings", get(page))
        .route("/settings/theme", post(select_theme))
        .route("/settings/theme/delete", post(remove_theme))
        .route("/settings/generate", post(generate))
}

pub struct ThemeCard {
    pub name: String,
    pub mode: String,
    pub swatches: Vec<String>,
    pub custom: bool,
    pub current: bool,
}

#[derive(Template)]
#[template(path = "settings.html")]
struct SettingsPage {
    layout: Layout,
    themes: Vec<ThemeCard>,
    current: String,
    light_count: usize,
    dark_count: usize,
    /// LLM providers for theme generation, the default one preselected
    providers: Vec<super::chat::ProviderChoice>,
}

fn card(t: &theme::Theme, custom: bool, current: &str) -> ThemeCard {
    ThemeCard {
        name: t.name.clone(),
        mode: t.mode.clone(),
        swatches: ["primary", "secondary", "accent", "base-100"]
            .iter()
            .filter_map(|k| t.colors.get(*k))
            // Only plain hex colors end up in style attributes
            .filter(|c| c.starts_with('#') && c.len() <= 9 && c[1..].chars().all(|ch| ch.is_ascii_hexdigit()))
            .cloned()
            .collect(),
        custom,
        current: t.name.eq_ignore_ascii_case(current),
    }
}

async fn page(State(state): State<AppState>, Extension(user): Extension<AuthUser>) -> WebResult<Html<String>> {
    let Json(settings) = get_settings(State(state.clone()), Extension(user.clone())).await?;
    let mut themes: Vec<ThemeCard> =
        settings.custom_themes.iter().map(|c| card(&theme::Theme::from(c), true, &settings.theme_name)).collect();
    themes.extend(theme::builtin_themes().iter().map(|t| card(t, false, &settings.theme_name)));
    let light_count = themes.iter().filter(|t| t.mode == "light").count();
    let dark_count = themes.len() - light_count;
    let current = themes.iter().find(|t| t.current).map(|t| t.name.clone()).unwrap_or(settings.theme_name);
    let Json(providers) = list_providers(State(state.clone()), Extension(user.clone())).await?;
    let providers = providers
        .into_iter()
        .map(|p| super::chat::ProviderChoice {
            selected: p.is_default,
            id: p.id,
            label: format!("{} · {}", p.name, p.model.unwrap_or_default()),
        })
        .collect();
    render(SettingsPage { layout: Layout::for_user(&state, &user, "").await?, themes, current, light_count, dark_count, providers })
}

/// Reload the page so it is rendered with the new theme
fn refresh() -> Response {
    ([("HX-Refresh", "true")], "").into_response()
}

#[derive(Deserialize)]
struct ThemeForm {
    theme_name: String,
}

async fn select_theme(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Form(form): Form<ThemeForm>,
) -> WebResult<Response> {
    let _ = update_settings(State(state), Extension(user), Json(UpdateSettingsRequest { theme_name: form.theme_name })).await?;
    Ok(refresh())
}

async fn remove_theme(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Form(form): Form<ThemeForm>,
) -> WebResult<Response> {
    let _ = delete_theme(State(state), Extension(user), Json(DeleteThemeRequest { theme_name: form.theme_name })).await?;
    Ok(refresh())
}

#[derive(Deserialize)]
struct GenerateForm {
    #[serde(default)]
    prompt: String,
    #[serde(default)]
    name: String,
    /// Empty: the default provider
    #[serde(default)]
    provider_id: String,
}

async fn generate(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Form(form): Form<GenerateForm>,
) -> WebResult<Response> {
    let error = |message: String| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            [("HX-Retarget", "#generate-error"), ("HX-Reswap", "innerHTML")],
            askama::filters::escape(&message, askama::filters::Html).map(|s| s.to_string()).unwrap_or_default(),
        )
            .into_response()
    };
    if form.prompt.trim().is_empty() {
        return Ok(error("Describe the theme you want.".into()));
    }
    let request = GenerateThemeRequest {
        prompt: form.prompt,
        custom_name: Some(form.name.trim().to_string()).filter(|n| !n.is_empty()),
        provider_id: form.provider_id.trim().parse().ok(),
    };
    match generate_theme(State(state.clone()), Extension(user.clone()), Json(request)).await {
        Ok(Json(generated)) => {
            // Use the new theme right away
            let _ = update_settings(State(state), Extension(user), Json(UpdateSettingsRequest { theme_name: generated.theme.name })).await?;
            Ok(refresh())
        }
        Err(e) => {
            tracing::warn!("Theme generation failed for user {}: {}", user.user_id, e);
            Ok(error(user_message(&e)))
        }
    }
}
