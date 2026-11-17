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

    // Get user's default LLM provider
    use crate::models::llm_provider::LlmProvider;

    let provider = sqlx::query_as::<_, LlmProvider>(
        "SELECT * FROM llm_providers WHERE user_id = ? AND is_default = 1 LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::BadRequest("No default LLM provider configured".to_string()))?;

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

    // Call LLM using generic client
    let completion = llm_client
        .chat_completion(messages, None, Some(0.7), Some(1024))
        .await?;

    // Extract response text
    let response_text = completion.choices
        .get(0)
        .and_then(|choice| choice.message.content.clone())
        .ok_or_else(|| AppError::BadRequest("Empty response from LLM".to_string()))?;

    // Parse JSON response
    let mut theme: CustomTheme = serde_json::from_str(&response_text)
        .map_err(|e| AppError::BadRequest(format!("Failed to parse theme: {}", e)))?;

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
