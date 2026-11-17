use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct LlmProvider {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub base_url: String,
    #[serde(skip_serializing)]
    pub api_key_encrypted: String,
    pub model: Option<String>,
    pub is_default: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Tokens per minute reported by the provider's rate-limit headers
    pub rate_limit_tokens: Option<i64>,
    pub rate_limit_checked_at: Option<DateTime<Utc>>,
    /// Cap for a tool result sent back to the LLM (None = automatic)
    pub max_tool_result_tokens: Option<i64>,
}

/// Store the tokens-per-minute limit a provider reported, if it changed.
pub async fn record_rate_limit(
    db: &sqlx::SqlitePool,
    provider: &LlmProvider,
    limit_tokens: Option<i64>,
) -> Result<(), sqlx::Error> {
    if let Some(limit) = limit_tokens {
        if provider.rate_limit_tokens != Some(limit) || provider.rate_limit_checked_at.is_none() {
            sqlx::query(
                "UPDATE llm_providers SET rate_limit_tokens = ?, rate_limit_checked_at = CURRENT_TIMESTAMP WHERE id = ?",
            )
            .bind(limit)
            .bind(provider.id)
            .execute(db)
            .await?;
        }
    }
    Ok(())
}

impl LlmProvider {
    /// The decrypted API key; providers without a key (local Ollama, LM
    /// Studio) have an empty one, which is not encrypted.
    pub fn api_key(&self, config: &crate::config::Config) -> crate::error::AppResult<String> {
        if self.api_key_encrypted.is_empty() {
            return Ok(String::new());
        }
        crate::services::encryption::EncryptionService::from_config(config)
            .decrypt(&self.api_key_encrypted)
            .map_err(|e| crate::error::AppError::Internal(format!("Failed to decrypt API key: {e}")))
    }

    /// Cap for tool results sent to this provider: the configured value, else
    /// half of the reported tokens-per-minute limit, else no cap.
    pub fn tool_result_token_cap(&self) -> Option<usize> {
        self.max_tool_result_tokens
            .or(self.rate_limit_tokens.map(|limit| limit / 2))
            .filter(|cap| *cap > 0)
            .map(|cap| cap as usize)
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateLlmProviderRequest {
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub model: Option<String>,
    pub is_default: bool,
}

#[derive(Debug, Deserialize)]
pub struct UpdateLlmProviderRequest {
    pub name: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub is_default: Option<bool>,
    /// Tool result cap in tokens; 0 = automatic (clears the setting)
    pub max_tool_result_tokens: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct LlmProviderResponse {
    pub id: i64,
    pub name: String,
    pub base_url: String,
    pub model: Option<String>,
    pub is_default: bool,
    pub has_api_key: bool,
    pub created_at: DateTime<Utc>,
    pub rate_limit_tokens: Option<i64>,
    pub rate_limit_checked_at: Option<DateTime<Utc>>,
    pub max_tool_result_tokens: Option<i64>,
    /// Effective cap used for tool results (configured or automatic)
    pub tool_result_token_cap: Option<usize>,
}

impl From<LlmProvider> for LlmProviderResponse {
    fn from(provider: LlmProvider) -> Self {
        let tool_result_token_cap = provider.tool_result_token_cap();
        LlmProviderResponse {
            tool_result_token_cap,
            created_at: provider.created_at,
            rate_limit_tokens: provider.rate_limit_tokens,
            rate_limit_checked_at: provider.rate_limit_checked_at,
            max_tool_result_tokens: provider.max_tool_result_tokens,
            id: provider.id,
            name: provider.name,
            base_url: provider.base_url,
            model: provider.model,
            is_default: provider.is_default,
            has_api_key: !provider.api_key_encrypted.is_empty(),
        }
    }
}
