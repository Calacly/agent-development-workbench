use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct UserSettings {
    pub user_id: i64,
    pub theme_name: String,
    pub custom_themes: Option<String>, // JSON array of custom themes
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UserSettingsResponse {
    pub theme_name: String,
    pub custom_themes: Vec<CustomTheme>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomTheme {
    pub name: String,
    pub mode: String, // "light" or "dark"
    pub colors: ThemeColors,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeColors {
    pub primary: String,
    pub secondary: String,
    pub accent: String,
    pub neutral: String,
    pub base_100: String,
    pub base_200: String,
    pub base_300: String,
    pub info: String,
    pub success: String,
    pub warning: String,
    pub error: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateSettingsRequest {
    pub theme_name: String,
}

#[derive(Debug, Deserialize)]
pub struct GenerateThemeRequest {
    pub prompt: String,
    pub custom_name: Option<String>,
    /// LLM provider to use; the default provider when missing
    #[serde(default)]
    pub provider_id: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct GenerateThemeResponse {
    pub theme: CustomTheme,
}

#[derive(Debug, Deserialize)]
pub struct DeleteThemeRequest {
    pub theme_name: String,
}
