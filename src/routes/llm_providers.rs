use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Extension, Json, Router,
};

use crate::{
    error::{AppError, AppResult},
    middleware::auth::AuthUser,
    models::llm_provider::{
        CreateLlmProviderRequest, LlmProvider, LlmProviderResponse, UpdateLlmProviderRequest,
    },
    services::{
        encryption::EncryptionService,
        llm_client::{LlmClient, Message},
    },
    AppState,
};
use serde::Deserialize;
use std::time::{Duration, Instant};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_providers).post(create_provider))
        .route("/:id", get(get_provider).put(update_provider).delete(delete_provider))
        .route("/models", post(list_models_for_config))
        .route("/test", post(test_config))
        .route("/:id/test", post(test_connection))
}

/// Load a provider owned by the user and decrypt its API key.
async fn load_provider_with_key(
    state: &AppState,
    user_id: i64,
    id: i64,
) -> AppResult<(LlmProvider, String)> {
    let provider = sqlx::query_as::<_, LlmProvider>(
        "SELECT * FROM llm_providers WHERE id = ? AND user_id = ?",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("Provider not found".to_string()))?;

    let api_key = if provider.api_key_encrypted.is_empty() {
        String::new()
    } else {
        EncryptionService::from_config(&state.config).decrypt(&provider.api_key_encrypted)?
    };
    Ok((provider, api_key))
}

#[derive(Debug, Deserialize)]
pub(crate) struct ListModelsRequest {
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
}

/// List models for a configuration that is not saved yet (the "Add provider" form).
pub(crate) async fn list_models_for_config(
    Extension(_auth_user): Extension<AuthUser>,
    Json(payload): Json<ListModelsRequest>,
) -> AppResult<Json<serde_json::Value>> {
    if payload.base_url.trim().is_empty() {
        return Err(AppError::BadRequest("base_url is required".to_string()));
    }
    let models = LlmClient::list_models(&payload.base_url, &payload.api_key).await?;
    Ok(Json(serde_json::json!({ "models": models })))
}

pub(crate) async fn list_providers(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthUser>,
) -> AppResult<Json<Vec<LlmProviderResponse>>> {
    let providers = sqlx::query_as::<_, LlmProvider>(
        "SELECT * FROM llm_providers WHERE user_id = ? ORDER BY created_at DESC",
    )
    .bind(auth_user.user_id)
    .fetch_all(&state.db)
    .await?;

    let response: Vec<LlmProviderResponse> = providers
        .into_iter()
        .map(|p| p.into())
        .collect();

    Ok(Json(response))
}

pub(crate) async fn create_provider(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthUser>,
    Json(payload): Json<CreateLlmProviderRequest>,
) -> AppResult<(StatusCode, Json<LlmProviderResponse>)> {
    // Validate input
    if payload.name.trim().is_empty() {
        return Err(AppError::BadRequest("Provider name cannot be empty".to_string()));
    }
    if payload.base_url.trim().is_empty() {
        return Err(AppError::BadRequest("Base URL cannot be empty".to_string()));
    }

    // Encrypt API key
    let encryption = EncryptionService::from_config(&state.config);
    let encrypted_key = if !payload.api_key.is_empty() {
        encryption.encrypt(&payload.api_key)?
    } else {
        String::new()
    };

    // If this is set as default, unset other defaults
    if payload.is_default {
        sqlx::query("UPDATE llm_providers SET is_default = 0 WHERE user_id = ?")
            .bind(auth_user.user_id)
            .execute(&state.db)
            .await?;
    }

    // Create provider
    let provider = sqlx::query_as::<_, LlmProvider>(
        "INSERT INTO llm_providers (user_id, name, base_url, api_key_encrypted, model, is_default)
         VALUES (?, ?, ?, ?, ?, ?) RETURNING *",
    )
    .bind(auth_user.user_id)
    .bind(&payload.name)
    .bind(&payload.base_url)
    .bind(&encrypted_key)
    .bind(&payload.model)
    .bind(if payload.is_default { 1 } else { 0 })
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(provider.into())))
}

pub(crate) async fn get_provider(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> AppResult<Json<LlmProviderResponse>> {
    let provider = sqlx::query_as::<_, LlmProvider>(
        "SELECT * FROM llm_providers WHERE id = ? AND user_id = ?",
    )
    .bind(id)
    .bind(auth_user.user_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("Provider not found".to_string()))?;

    Ok(Json(provider.into()))
}

pub(crate) async fn update_provider(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthUser>,
    Path(id): Path<i64>,
    Json(payload): Json<UpdateLlmProviderRequest>,
) -> AppResult<Json<LlmProviderResponse>> {
    // Check if provider exists and belongs to user
    let existing = sqlx::query_as::<_, LlmProvider>(
        "SELECT * FROM llm_providers WHERE id = ? AND user_id = ?",
    )
    .bind(id)
    .bind(auth_user.user_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("Provider not found".to_string()))?;

    // If setting as default, unset other defaults
    if payload.is_default == Some(true) {
        sqlx::query("UPDATE llm_providers SET is_default = 0 WHERE user_id = ? AND id != ?")
            .bind(auth_user.user_id)
            .bind(id)
            .execute(&state.db)
            .await?;
    }

    // Handle API key encryption if provided
    let encrypted_key = if let Some(ref api_key) = payload.api_key {
        if !api_key.is_empty() {
            let encryption = EncryptionService::from_config(&state.config);
            Some(encryption.encrypt(api_key)?)
        } else {
            Some(String::new())
        }
    } else {
        None
    };

    // Build update query dynamically
    let mut query = String::from("UPDATE llm_providers SET ");
    let mut updates = Vec::new();
    let mut params: Vec<String> = Vec::new();

    if let Some(name) = &payload.name {
        updates.push("name = ?");
        params.push(name.clone());
    }
    if let Some(base_url) = &payload.base_url {
        updates.push("base_url = ?");
        params.push(base_url.clone());
    }
    if let Some(key) = encrypted_key {
        updates.push("api_key_encrypted = ?");
        params.push(key);
    }
    if let Some(model) = &payload.model {
        updates.push("model = ?");
        params.push(model.clone());
    }
    if let Some(is_default) = payload.is_default {
        updates.push("is_default = ?");
        params.push(if is_default { "1" } else { "0" }.to_string());
    }
    match payload.max_tool_result_tokens {
        Some(tokens) if tokens < 0 => {
            return Err(AppError::BadRequest("max_tool_result_tokens must not be negative".to_string()));
        }
        // 0 = automatic: clear the setting
        Some(0) => updates.push("max_tool_result_tokens = NULL"),
        Some(tokens) => {
            updates.push("max_tool_result_tokens = ?");
            params.push(tokens.to_string());
        }
        None => {}
    }

    if updates.is_empty() {
        return Ok(Json(existing.into()));
    }

    query.push_str(&updates.join(", "));
    query.push_str(" WHERE id = ? AND user_id = ?");

    let mut query_builder = sqlx::query(&query);
    for param in params {
        query_builder = query_builder.bind(param);
    }
    query_builder = query_builder.bind(id).bind(auth_user.user_id);

    query_builder.execute(&state.db).await?;

    // Fetch updated provider
    let provider = sqlx::query_as::<_, LlmProvider>(
        "SELECT * FROM llm_providers WHERE id = ? AND user_id = ?",
    )
    .bind(id)
    .bind(auth_user.user_id)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(provider.into()))
}

pub(crate) async fn delete_provider(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let result = sqlx::query("DELETE FROM llm_providers WHERE id = ? AND user_id = ?")
        .bind(id)
        .bind(auth_user.user_id)
        .execute(&state.db)
        .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("Provider not found".to_string()));
    }

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub(crate) struct TestConfigRequest {
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub model: Option<String>,
}

/// Test a configuration that is not saved yet (the "Add provider" dialog).
pub(crate) async fn test_config(
    Extension(_auth_user): Extension<AuthUser>,
    Json(payload): Json<TestConfigRequest>,
) -> AppResult<Json<serde_json::Value>> {
    if payload.base_url.trim().is_empty() {
        return Err(AppError::BadRequest("base_url is required".to_string()));
    }
    Ok(Json(hello_test(payload.base_url, payload.api_key, payload.model).await))
}

/// Test a saved provider with its stored API key and model.
pub(crate) async fn test_connection(
    State(state): State<AppState>,
    Extension(auth_user): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> AppResult<Json<serde_json::Value>> {
    let (provider, api_key) = load_provider_with_key(&state, auth_user.user_id, id).await?;
    let result = hello_test(provider.base_url.clone(), api_key, provider.model.clone()).await;
    // Remember the reported tokens-per-minute limit for the provider card
    let limit = result.get("rate_limit_tokens").and_then(|v| v.as_i64());
    crate::models::llm_provider::record_rate_limit(&state.db, &provider, limit).await?;
    Ok(Json(result))
}

/// Send a short "hello" chat completion. Always returns a body with `success`,
/// so the UI can show the provider's own error text.
async fn hello_test(base_url: String, api_key: String, model: Option<String>) -> serde_json::Value {
    let Some(model) = model.filter(|m| !m.trim().is_empty()) else {
        return serde_json::json!({ "success": false, "message": "No model configured" });
    };

    let client = LlmClient::new(base_url, api_key, model.clone());
    let messages = vec![Message {
        role: "user".to_string(),
        content: Some("hello".to_string()),
        tool_calls: None,
        tool_call_id: None,
    }];

    let started = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        client.chat_completion(messages, None, None, Some(64)),
    )
    .await;
    let latency_ms = started.elapsed().as_millis() as u64;

    match result {
        Err(_) => serde_json::json!({
            "success": false,
            "message": "Timed out after 30 s",
            "model": model,
            "latency_ms": latency_ms
        }),
        Ok(Err(e)) => serde_json::json!({
            "success": false,
            "message": e.to_string(),
            "model": model,
            "latency_ms": latency_ms
        }),
        Ok(Ok(completion)) => {
            let reply = completion
                .choices
                .first()
                .and_then(|c| c.message.content.clone())
                .unwrap_or_default();
            serde_json::json!({
                "success": true,
                "message": "Provider answered",
                "reply": reply,
                "model": completion.model,
                "latency_ms": latency_ms,
                "rate_limit_tokens": completion.rate_limit.limit_tokens,
                "remaining_tokens": completion.rate_limit.remaining_tokens
            })
        }
    }
}
