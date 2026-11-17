//! LLM Providers page. Handlers call the JSON API handlers in
//! routes::llm_providers, so validation, encryption and the LLM calls stay in
//! one place; this module only turns their results into HTML.

use askama::Template;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Extension, Form, Json, Router,
};
use serde::Deserialize;

use super::{format_number, render, user_message, Layout, WebResult};
use crate::{
    middleware::auth::AuthUser,
    models::llm_provider::{CreateLlmProviderRequest, LlmProviderResponse, UpdateLlmProviderRequest},
    routes::llm_providers::{
        create_provider, delete_provider, get_provider, list_models_for_config, list_providers, test_config,
        test_connection, update_provider, ListModelsRequest, TestConfigRequest,
    },
    AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/llm-providers", get(page).post(create))
        .route("/llm-providers/models", post(load_models))
        .route("/llm-providers/test", post(test_unsaved))
        .route("/llm-providers/:id", axum::routing::delete(delete))
        .route("/llm-providers/:id/test", post(test_saved))
        .route("/llm-providers/:id/default", post(set_default))
        .route("/llm-providers/:id/cap", post(set_cap))
}

// ---------- View models ----------

pub struct Preset {
    pub label: &'static str,
    pub name: &'static str,
    pub base_url: &'static str,
    pub model: &'static str,
}

const PRESETS: &[Preset] = &[
    Preset { label: "OpenAI", name: "OpenAI", base_url: "https://api.openai.com/v1", model: "gpt-4o" },
    Preset { label: "Anthropic (Claude)", name: "Anthropic", base_url: "https://api.anthropic.com/v1", model: "claude-sonnet-5-5" },
    Preset { label: "Google AI (Gemini)", name: "Google AI", base_url: "https://generativelanguage.googleapis.com/v1beta", model: "gemini-2.5-flash-lite" },
    Preset { label: "Mistral AI", name: "Mistral AI", base_url: "https://api.mistral.ai/v1", model: "mistral-large-latest" },
    Preset { label: "Groq", name: "Groq", base_url: "https://api.groq.com/openai/v1", model: "llama-3.3-70b-versatile" },
    Preset { label: "OpenRouter", name: "OpenRouter", base_url: "https://openrouter.ai/api/v1", model: "anthropic/claude-3.5-sonnet" },
    Preset { label: "Together AI", name: "Together AI", base_url: "https://api.together.xyz/v1", model: "meta-llama/Meta-Llama-3.1-70B-Instruct-Turbo" },
    Preset { label: "Perplexity", name: "Perplexity", base_url: "https://api.perplexity.ai", model: "llama-3.1-sonar-large-128k-online" },
    Preset { label: "DeepSeek", name: "DeepSeek", base_url: "https://api.deepseek.com/v1", model: "deepseek-chat" },
    Preset { label: "Ollama (local)", name: "Ollama", base_url: "http://localhost:11434/v1", model: "llama3.2" },
    Preset { label: "LM Studio (local)", name: "LM Studio", base_url: "http://localhost:1234/v1", model: "local-model" },
];

/// One provider card
pub struct ProviderView {
    pub id: i64,
    pub name: String,
    pub base_url: String,
    pub model: String,
    pub is_default: bool,
    pub limit: Option<String>,
    pub limit_checked: Option<String>,
    pub cap_value: String,
    pub cap_help: String,
    pub cap_error: Option<String>,
    pub created: String,
}

impl From<LlmProviderResponse> for ProviderView {
    fn from(p: LlmProviderResponse) -> Self {
        let cap_help = match (p.max_tool_result_tokens, p.tool_result_token_cap) {
            (Some(_), _) => "Longer tool results are shortened before they go to the LLM.".to_string(),
            (None, Some(cap)) => format!("Automatic: {}, half the token limit.", format_number(cap as i64)),
            (None, None) => "Automatic: no cap until the token limit is known.".to_string(),
        };
        ProviderView {
            id: p.id,
            name: p.name,
            base_url: p.base_url,
            model: p.model.unwrap_or_default(),
            is_default: p.is_default,
            limit: p.rate_limit_tokens.map(format_number),
            limit_checked: p.rate_limit_checked_at.map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string()),
            cap_value: p.max_tool_result_tokens.map(|c| c.to_string()).unwrap_or_default(),
            cap_help,
            cap_error: None,
            created: p.created_at.format("%Y-%m-%d").to_string(),
        }
    }
}

#[derive(Template)]
#[template(path = "llm_providers.html")]
struct ProvidersPage {
    layout: Layout,
    providers: Vec<ProviderView>,
    presets: &'static [Preset],
}

#[derive(Template)]
#[template(path = "partials/provider_list.html")]
struct ProviderList {
    providers: Vec<ProviderView>,
}

#[derive(Template)]
#[template(path = "partials/provider_card.html")]
struct ProviderCard {
    p: ProviderView,
}

#[derive(Template)]
#[template(path = "partials/test_result.html")]
struct TestResult {
    target_id: String,
    card_wide: bool,
    success: bool,
    model: String,
    latency_ms: u64,
    reply: String,
    message: String,
    limit: Option<String>,
    remaining: Option<String>,
    /// Card to update out of band (the saved provider's token limit)
    card: Option<ProviderView>,
}

#[derive(Template)]
#[template(path = "partials/model_field.html")]
struct ModelField {
    models: Vec<String>,
    current: String,
    message: String,
    is_error: bool,
}

// ---------- Helpers ----------

async fn providers(state: &AppState, user: &AuthUser) -> WebResult<Vec<ProviderView>> {
    let Json(list) = list_providers(State(state.clone()), Extension(user.clone())).await?;
    Ok(list.into_iter().map(ProviderView::from).collect())
}

async fn list_fragment(state: &AppState, user: &AuthUser) -> WebResult<Html<String>> {
    render(ProviderList { providers: providers(state, user).await? })
}

/// Validation error for a dialog: 422, placed into `target` (see app.js)
fn form_error(target: &str, message: &str) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        [("HX-Retarget", target), ("HX-Reswap", "innerHTML")],
        askama::filters::escape(message, askama::filters::Html).map(|s| s.to_string()).unwrap_or_default(),
    )
        .into_response()
}

fn empty_to_none(value: Option<String>) -> Option<String> {
    value.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn test_result_view(target_id: String, card_wide: bool, value: &serde_json::Value, card: Option<ProviderView>) -> TestResult {
    let text = |key: &str| value.get(key).and_then(|v| v.as_str()).unwrap_or_default().to_string();
    let number = |key: &str| value.get(key).and_then(|v| v.as_i64()).map(format_number);
    TestResult {
        target_id,
        card_wide,
        success: value.get("success").and_then(|v| v.as_bool()).unwrap_or(false),
        model: text("model"),
        latency_ms: value.get("latency_ms").and_then(|v| v.as_u64()).unwrap_or(0),
        reply: text("reply"),
        message: text("message"),
        limit: number("rate_limit_tokens"),
        remaining: number("remaining_tokens"),
        card,
    }
}

// ---------- Handlers ----------

async fn page(State(state): State<AppState>, Extension(user): Extension<AuthUser>) -> WebResult<Html<String>> {
    render(ProvidersPage {
        layout: Layout::for_user(&state, &user, "/llm-providers").await?,
        providers: providers(&state, &user).await?,
        presets: PRESETS,
    })
}

#[derive(Deserialize)]
struct ProviderForm {
    #[serde(default)]
    name: String,
    #[serde(default)]
    base_url: String,
    #[serde(default)]
    api_key: String,
    model: Option<String>,
}

async fn create(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Form(form): Form<ProviderForm>,
) -> WebResult<Response> {
    let name = form.name.trim().to_string();
    let base_url = form.base_url.trim().to_string();
    if name.is_empty() || base_url.is_empty() {
        return Ok(form_error("#add-provider-error", "Name and base URL are required."));
    }
    let model = empty_to_none(form.model).filter(|m| m != "__other__");
    if model.is_none() {
        return Ok(form_error("#add-provider-error", "Enter a model, or load the provider's models and pick one."));
    }
    // The first provider becomes the default, so the chat works right away
    let is_default = providers(&state, &user).await?.is_empty();
    let request = CreateLlmProviderRequest { name, base_url, api_key: form.api_key, model, is_default };
    match create_provider(State(state.clone()), Extension(user.clone()), Json(request)).await {
        Ok(_) => Ok(list_fragment(&state, &user).await?.into_response()),
        Err(e) => Ok(form_error("#add-provider-error", &user_message(&e))),
    }
}

async fn load_models(Extension(user): Extension<AuthUser>, Form(form): Form<ProviderForm>) -> WebResult<Html<String>> {
    let current = empty_to_none(form.model).unwrap_or_default();
    if form.base_url.trim().is_empty() {
        return render(ModelField { models: vec![], current, message: "Enter a base URL first.".into(), is_error: true });
    }
    let request = ListModelsRequest { base_url: form.base_url.trim().to_string(), api_key: form.api_key };
    match list_models_for_config(Extension(user), Json(request)).await {
        Ok(Json(value)) => {
            let models: Vec<String> = value["models"]
                .as_array()
                .map(|a| a.iter().filter_map(|m| m.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            if models.is_empty() {
                return render(ModelField { models, current, message: "The provider returned no models.".into(), is_error: false });
            }
            // Keep the preset's model if the provider has it, else the first one
            let current = if models.contains(&current) { current } else { models[0].clone() };
            let message = format!("{} models loaded. Choose one, or Other to type a name.", models.len());
            render(ModelField { models, current, message, is_error: false })
        }
        Err(e) => render(ModelField { models: vec![], current, message: user_message(&e), is_error: true }),
    }
}

async fn test_unsaved(Extension(user): Extension<AuthUser>, Form(form): Form<ProviderForm>) -> WebResult<Html<String>> {
    let value = if form.base_url.trim().is_empty() {
        serde_json::json!({ "success": false, "message": "Enter a base URL and a model first." })
    } else {
        let request = TestConfigRequest {
            base_url: form.base_url.trim().to_string(),
            api_key: form.api_key,
            model: empty_to_none(form.model).filter(|m| m != "__other__"),
        };
        match test_config(Extension(user), Json(request)).await {
            Ok(Json(value)) => value,
            Err(e) => serde_json::json!({ "success": false, "message": user_message(&e) }),
        }
    };
    render(test_result_view("add-provider-test".into(), false, &value, None))
}

async fn test_saved(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> WebResult<Html<String>> {
    let value = match test_connection(State(state.clone()), Extension(user.clone()), Path(id)).await {
        Ok(Json(value)) => value,
        Err(e) => serde_json::json!({ "success": false, "message": user_message(&e) }),
    };
    // The test stores the reported token limit; show it on the card right away
    let card = get_provider(State(state), Extension(user), Path(id)).await.ok().map(|Json(p)| ProviderView::from(p));
    render(test_result_view(format!("test-{id}"), true, &value, card))
}

async fn set_default(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> WebResult<Html<String>> {
    let request = UpdateLlmProviderRequest {
        name: None,
        base_url: None,
        api_key: None,
        model: None,
        is_default: Some(true),
        max_tool_result_tokens: None,
    };
    let _ = update_provider(State(state.clone()), Extension(user.clone()), Path(id), Json(request)).await?;
    list_fragment(&state, &user).await
}

async fn delete(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> WebResult<Html<String>> {
    delete_provider(State(state.clone()), Extension(user.clone()), Path(id)).await?;
    list_fragment(&state, &user).await
}

#[derive(Deserialize)]
struct CapForm {
    #[serde(default)]
    cap: String,
}

async fn set_cap(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
    Form(form): Form<CapForm>,
) -> WebResult<Response> {
    let value = form.cap.trim();
    // Empty = automatic (0 clears the setting in the API)
    let tokens = if value.is_empty() { Ok(0) } else { value.parse::<i64>().map_err(|_| ()) };
    match tokens {
        Ok(tokens) if tokens >= 0 => {
            let request = UpdateLlmProviderRequest {
                name: None,
                base_url: None,
                api_key: None,
                model: None,
                is_default: None,
                max_tool_result_tokens: Some(tokens),
            };
            let Json(provider) = update_provider(State(state), Extension(user), Path(id), Json(request)).await?;
            Ok(render(ProviderCard { p: provider.into() })?.into_response())
        }
        _ => {
            let Json(provider) = get_provider(State(state), Extension(user), Path(id)).await?;
            let mut p = ProviderView::from(provider);
            p.cap_value = value.to_string();
            p.cap_error = Some("Enter a whole number of tokens, or leave it empty for automatic.".into());
            Ok((StatusCode::UNPROCESSABLE_ENTITY, render(ProviderCard { p })?).into_response())
        }
    }
}
