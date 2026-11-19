use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredToolCall {
    pub tool_name: String,
    pub arguments: serde_json::Value,
    pub result: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Conversation {
    pub id: i64,
    pub user_id: i64,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// LLM provider used for the last message (may point to a deleted provider)
    #[sqlx(default)]
    pub last_provider_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Message {
    pub id: i64,
    pub conversation_id: i64,
    pub role: String, // "user", "assistant", "system"
    pub content: String,
    #[sqlx(default)]
    pub tool_calls: Option<String>, // JSON string of tool calls array
    pub created_at: DateTime<Utc>,
    #[sqlx(default)]
    pub llm_steps: Option<String>, // JSON string of LlmStep array
}

/// One LLM round behind an assistant message, shown in the chat timeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmStep {
    /// 1-based round number
    pub round: u32,
    /// What was sent: model, total message count, the messages new since the
    /// previous round (long contents shortened) and the offered tool names
    pub request: serde_json::Value,
    /// What came back: text, requested tool calls, finish reason
    pub response: Option<serde_json::Value>,
    pub usage: Option<serde_json::Value>,
    pub duration_ms: u64,
    pub error: Option<String>,
    /// Indices into the message's tool_calls that this round triggered
    pub tool_call_indices: Vec<usize>,
}

#[derive(Debug, Deserialize)]
pub struct CreateConversationRequest {
    pub title: String,
}

/// One of the user's MCP servers as seen from a conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationServer {
    pub id: i64,
    pub name: String,
    /// Connected right now (only connected servers offer tools)
    pub connected: bool,
    /// Switched on for this conversation
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct SetServerEnabledRequest {
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct SendMessageRequest {
    pub content: String,
    pub provider_id: Option<i64>,
}
