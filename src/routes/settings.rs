use axum::{
    extract::State,
    routing::{delete, get, post},
    Extension, Json, Router,
};

use crate::{
    error::{AppError, AppResult},
    middleware::auth::AuthUser,
    models::user_settings::{
        CustomTheme, DeleteThemeRequest, GenerateThemeRequest, GenerateThemeResponse,
        UpdateSettingsRequest, UserSettings, UserSettingsResponse,
    },
    services::llm_client::{LlmClient, Message},
    AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(get_settings).put(update_settings))
        .route("/generate-theme", post(generate_theme))
        .route("/theme", delete(delete_theme))
}

// Get user settings
pub(crate) async fn get_settings(
    State(state): State<AppState>,
    Extension(AuthUser { user_id, .. }): Extension<AuthUser>,
) -> AppResult<Json<UserSettingsResponse>> {
    // Try to get existing settings
    let settings = sqlx::query_as::<_, UserSettings>(
        "SELECT * FROM user_settings WHERE user_id = ?",
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?;

    let settings = match settings {
        Some(s) => s,
        None => {
            // Create default settings
            let default_settings = sqlx::query_as::<_, UserSettings>(
                "INSERT INTO user_settings (user_id, theme_name) VALUES (?, 'light') RETURNING *",
            )
            .bind(user_id)
            .fetch_one(&state.db)
            .await?;
            default_settings
        }
    };

    // Parse custom themes
    let custom_themes: Vec<CustomTheme> = if let Some(themes_json) = &settings.custom_themes {
        serde_json::from_str(themes_json).unwrap_or_default()
    } else {
        vec![]
    };

    Ok(Json(UserSettingsResponse {
        theme_name: settings.theme_name,
        custom_themes,
    }))
}

// Update user settings
pub(crate) async fn update_settings(
    State(state): State<AppState>,
    Extension(AuthUser { user_id, .. }): Extension<AuthUser>,
    Json(req): Json<UpdateSettingsRequest>,
) -> AppResult<Json<UserSettingsResponse>> {
    // Ensure settings exist
    sqlx::query(
        "INSERT INTO user_settings (user_id, theme_name) VALUES (?, ?)
         ON CONFLICT(user_id) DO UPDATE SET theme_name = ?, updated_at = CURRENT_TIMESTAMP",
    )
    .bind(user_id)
    .bind(&req.theme_name)
    .bind(&req.theme_name)
    .execute(&state.db)
    .await?;

    // Return updated settings
    get_settings(State(state), Extension(AuthUser { user_id, username: String::new() })).await
}

/// The JSON object in a model answer: models often wrap it in a ```json fence,
/// add a sentence around it, or put their thinking in <think> tags first
fn theme_json(text: &str) -> &str {
    let text = match text.rfind("</think>") {
        Some(end) => &text[end + "</think>".len()..],
        None => text,
    };
    match (text.find('{'), text.rfind('}')) {
        (Some(start), Some(end)) if start < end => &text[start..=end],
        _ => text.trim(),
    }
}

// Generate theme using LLM
pub(crate) async fn generate_theme(
    State(state): State<AppState>,
    Extension(AuthUser { user_id, .. }): Extension<AuthUser>,
    Json(req): Json<GenerateThemeRequest>,
) -> AppResult<Json<GenerateThemeResponse>> {
    // Build LLM prompt for theme generation
    let system_prompt = r##"You are a theme designer. Generate a beautiful, cohesive color theme based on the user's request.
Return ONLY a valid JSON object with this exact structure (no markdown, no explanation):
{
  "name": "theme name (short, 2-4 words)",
  "mode": "light or dark",
  "colors": {
    "primary": "#hexcolor",
    "secondary": "#hexcolor",
    "accent": "#hexcolor",
    "neutral": "#hexcolor",
    "base_100": "#hexcolor (lightest background)",
    "base_200": "#hexcolor (medium background)",
    "base_300": "#hexcolor (darkest background/border)",
    "info": "#hexcolor (blue-ish)",
    "success": "#hexcolor (green-ish)",
    "warning": "#hexcolor (yellow/orange-ish)",
    "error": "#hexcolor (red-ish)"
  }
}

For light themes: base_100 should be very light (near white), base_300 darker
For dark themes: base_100 should be dark (near black), base_300 lighter
Ensure good contrast for readability."##;

    // The chosen LLM provider, or the user's default one
    use crate::models::llm_provider::LlmProvider;

    let provider = match req.provider_id {
        Some(provider_id) => sqlx::query_as::<_, LlmProvider>("SELECT * FROM llm_providers WHERE id = ? AND user_id = ?")
            .bind(provider_id)
            .bind(user_id)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| AppError::NotFound("LLM provider not found".to_string()))?,
        None => sqlx::query_as::<_, LlmProvider>("SELECT * FROM llm_providers WHERE user_id = ? AND is_default = 1 LIMIT 1")
            .bind(user_id)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| AppError::BadRequest("No LLM provider chosen and no default configured".to_string()))?,
    };

    // Decrypt API key
    let api_key = provider.api_key(&state.config)?;

    // Get model or use a default
    let model = provider.model.unwrap_or_else(|| "gpt-4".to_string());

    // Use generic LLM client that works with any OpenAI-compatible API
    let llm_client = LlmClient::new(provider.base_url.clone(), api_key, model);

    // Build messages in OpenAI format
    let messages = vec![
        Message {
            role: "system".to_string(),
            content: Some(system_prompt.to_string()),
            tool_calls: None,
            tool_call_id: None,
        },
        Message {
            role: "user".to_string(),
            content: Some(req.prompt.clone()),
            tool_calls: None,
            tool_call_id: None,
        },
    ];

    // No max_tokens: thinking models (Qwen, GPT-OSS) spend hundreds of tokens
    // before the JSON and were cut off at the old limit of 1024
    let completion = llm_client
        .chat_completion(messages, None, Some(0.7), None)
        .await?;

    let choice = completion.choices.first();
    let response_text = choice
        .and_then(|choice| choice.message.content.clone())
        .unwrap_or_default();
    if choice.and_then(|c| c.finish_reason.as_deref()) == Some("length") {
        return Err(AppError::BadRequest(
            "The model stopped before the theme was complete (it reached its token limit). Try another model.".to_string(),
        ));
    }
    if response_text.trim().is_empty() {
        return Err(AppError::BadRequest("The model returned no theme. Try another model.".to_string()));
    }

    let mut theme: CustomTheme = serde_json::from_str(theme_json(&response_text))
        .map_err(|e| AppError::BadRequest(format!("The model did not return a valid theme: {e}")))?;

    // Override name if user provided one
    if let Some(custom_name) = req.custom_name {
        theme.name = format!("[custom] {}", custom_name);
    } else {
        theme.name = format!("[custom] {}", theme.name);
    }

    // Store the custom theme
    let settings = sqlx::query_as::<_, UserSettings>(
        "SELECT * FROM user_settings WHERE user_id = ?",
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?;

    let mut custom_themes: Vec<CustomTheme> = if let Some(s) = settings {
        if let Some(themes_json) = s.custom_themes {
            serde_json::from_str(&themes_json).unwrap_or_default()
        } else {
            vec![]
        }
    } else {
        vec![]
    };

    // Add new theme
    custom_themes.push(theme.clone());

    // Update settings
    let themes_json = serde_json::to_string(&custom_themes)?;
    sqlx::query(
        "INSERT INTO user_settings (user_id, custom_themes) VALUES (?, ?)
         ON CONFLICT(user_id) DO UPDATE SET custom_themes = ?, updated_at = CURRENT_TIMESTAMP",
    )
    .bind(user_id)
    .bind(&themes_json)
    .bind(&themes_json)
    .execute(&state.db)
    .await?;

    Ok(Json(GenerateThemeResponse { theme }))
}

// Delete a custom theme
pub(crate) async fn delete_theme(
    State(state): State<AppState>,
    Extension(AuthUser { user_id, .. }): Extension<AuthUser>,
    Json(req): Json<DeleteThemeRequest>,
) -> AppResult<Json<UserSettingsResponse>> {
    // Get current settings
    let settings = sqlx::query_as::<_, UserSettings>(
        "SELECT * FROM user_settings WHERE user_id = ?",
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?;

    let mut custom_themes: Vec<CustomTheme> = if let Some(s) = settings {
        if let Some(themes_json) = s.custom_themes {
            serde_json::from_str(&themes_json).unwrap_or_default()
        } else {
            vec![]
        }
    } else {
        vec![]
    };

    // Find and remove the theme
    let original_len = custom_themes.len();
    custom_themes.retain(|t| t.name != req.theme_name);

    if custom_themes.len() == original_len {
        return Err(AppError::BadRequest(format!(
            "Theme '{}' not found",
            req.theme_name
        )));
    }

    // Update settings in database
    let themes_json = serde_json::to_string(&custom_themes)?;
    sqlx::query(
        "UPDATE user_settings SET custom_themes = ?, updated_at = CURRENT_TIMESTAMP WHERE user_id = ?",
    )
    .bind(&themes_json)
    .bind(user_id)
    .execute(&state.db)
    .await?;

    // Return updated settings
    get_settings(
        State(state),
        Extension(AuthUser {
            user_id,
            username: String::new(),
        }),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::theme_json;

    #[test]
    fn finds_the_theme_json_in_model_answers() {
        let plain = r#"{"name":"A"}"#;
        assert_eq!(theme_json(plain), plain);
        assert_eq!(theme_json("```json\n{\"name\":\"A\"}\n```"), plain);
        assert_eq!(theme_json("Here is your theme:\n{\"name\":\"A\"}\nEnjoy!"), plain);
        assert_eq!(theme_json("<think>maybe {colors}</think>\n{\"name\":\"A\"}"), plain);
        assert_eq!(theme_json("no json"), "no json");
    }
}
