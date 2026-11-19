use crate::error::{AppError, AppResult};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use chrono::Local;
use std::fs;
use std::path::PathBuf;

/// API type for different LLM providers
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ApiType {
    OpenAI,  // Arguments as string
    Ollama,  // Arguments as object
}

/// Generic LLM client that works with OpenAI-compatible APIs
pub struct LlmClient {
    client: Client,
    base_url: String,
    api_key: String,
    model: String,
    api_type: ApiType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub r#type: String,
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    /// Arguments - stored as Value internally, can deserialize from string or object
    #[serde(deserialize_with = "deserialize_arguments")]
    pub arguments: serde_json::Value,
}

/// Deserialize arguments from either a JSON string or a JSON object
fn deserialize_arguments<'de, D>(deserializer: D) -> Result<serde_json::Value, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::String(s) => {
            // Parse the string as JSON
            serde_json::from_str(&s).map_err(|e| D::Error::custom(format!("Invalid JSON in arguments: {e}")))
        }
        other => Ok(other),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub r#type: String,
    pub function: ToolFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolFunction {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<Choice>,
    pub usage: Usage,
    /// Rate limits from the response headers (not part of the JSON body)
    #[serde(skip)]
    pub rate_limit: RateLimit,
}

/// Rate limits a provider reports in its response headers.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct RateLimit {
    /// Tokens per minute (Groq/OpenAI `x-ratelimit-limit-tokens`,
    /// Anthropic `anthropic-ratelimit-tokens-limit`)
    pub limit_tokens: Option<i64>,
    pub remaining_tokens: Option<i64>,
}

impl RateLimit {
    pub fn from_headers(headers: &reqwest::header::HeaderMap) -> Self {
        let number = |names: &[&str]| {
            names.iter().find_map(|name| {
                headers.get(*name)?.to_str().ok()?.trim().parse::<i64>().ok()
            })
        };
        Self {
            limit_tokens: number(&["x-ratelimit-limit-tokens", "anthropic-ratelimit-tokens-limit"]),
            remaining_tokens: number(&["x-ratelimit-remaining-tokens", "anthropic-ratelimit-tokens-remaining"]),
        }
    }
}

/// Longest message content kept in a step's request preview
const PREVIEW_CONTENT_CHARS: usize = 2000;

fn shorten(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        text.to_string()
    } else {
        let kept: String = text.chars().take(max_chars).collect();
        format!("{kept}… [{} more characters]", total - max_chars)
    }
}

/// Readable summary of a chat completion request for the chat timeline: the
/// messages from `new_from` on (what this round added), long contents
/// shortened, and only the names of the offered tools.
pub fn request_preview(model: &str, messages: &[Message], tools: &Option<Vec<Tool>>, new_from: usize) -> serde_json::Value {
    let new_messages: Vec<serde_json::Value> = messages
        .iter()
        .skip(new_from)
        .map(|m| {
            let mut value = serde_json::to_value(m).unwrap_or_default();
            if let Some(content) = m.content.as_deref() {
                value["content"] = serde_json::Value::String(shorten(content, PREVIEW_CONTENT_CHARS));
            }
            value
        })
        .collect();
    serde_json::json!({
        "model": model,
        "message_count": messages.len(),
        "new_messages": new_messages,
        "tools": tools.as_ref().map(|t| t.iter().map(|tool| tool.function.name.clone()).collect::<Vec<_>>()).unwrap_or_default(),
    })
}

/// Readable summary of a chat completion response for the chat timeline.
pub fn response_preview(response: &ChatCompletionResponse) -> (serde_json::Value, serde_json::Value) {
    let choice = response.choices.first();
    let tool_calls: Vec<serde_json::Value> = choice
        .and_then(|c| c.message.tool_calls.as_ref())
        .map(|calls| calls.iter().map(|c| serde_json::json!({ "name": c.function.name, "arguments": c.function.arguments })).collect())
        .unwrap_or_default();
    (
        serde_json::json!({
            "model": response.model,
            "content": choice.and_then(|c| c.message.content.clone()),
            "tool_calls": tool_calls,
            "finish_reason": choice.and_then(|c| c.finish_reason.clone()),
        }),
        serde_json::to_value(&response.usage).unwrap_or_default(),
    )
}

/// Approximate characters per token, used to cap tool results without a tokenizer
const CHARS_PER_TOKEN: usize = 4;

/// LLM rounds per chat message before the chat stops asking the model again
pub const MAX_LLM_ROUNDS: u32 = 10;

/// Answer saved when a chat message reaches `MAX_LLM_ROUNDS`
pub fn round_limit_message() -> String {
    format!("Stopped after {MAX_LLM_ROUNDS} LLM rounds without a final answer.")
}

/// Shorten a tool result sent to the LLM to about `max_tokens` tokens and say
/// so at the end, so large outputs (e.g. page snapshots) stay within the
/// provider's tokens-per-minute limit. The full result is still stored and shown.
pub fn cap_tool_result(text: &str, max_tokens: Option<usize>) -> String {
    let Some(max_tokens) = max_tokens else {
        return text.to_string();
    };
    let max_chars = max_tokens.saturating_mul(CHARS_PER_TOKEN);
    let total_chars = text.chars().count();
    if total_chars <= max_chars {
        return text.to_string();
    }
    let kept: String = text.chars().take(max_chars).collect();
    format!(
        "{kept}\n\n[Tool result truncated to about {max_tokens} tokens: {max_chars} of {total_chars} characters shown.]"
    )
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Choice {
    pub index: u32,
    pub message: ResponseMessage,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ResponseMessage {
    pub role: String,
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// Text to send back to the LLM for an MCP tool result: the joined `text`
/// items of its `content`, or the whole result as JSON when there are none.
pub fn tool_result_text(result: &serde_json::Value) -> String {
    let texts: Vec<&str> = result
        .get("content")
        .and_then(|c| c.as_array())
        .map(|items| items.iter().filter_map(|item| item.get("text")?.as_str()).collect())
        .unwrap_or_default();
    if texts.is_empty() {
        result.to_string()
    } else {
        texts.join("\n")
    }
}

impl LlmClient {
    pub fn new(base_url: String, api_key: String, model: String) -> Self {
        // The wire format depends on the server, not the model: open-weight models
        // (Qwen, Llama, Mistral, ...) behind OpenAI-compatible gateways such as
        // LiteLLM/vLLM, Groq or OpenRouter need OpenAI's string arguments. Only a
        // native Ollama endpoint gets arguments as an object.
        let api_type = Self::detect_api_type(&base_url);

        tracing::debug!("LLM client created with API type: {:?} for model: {}", api_type, model);

        Self {
            client: Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            model,
            api_type,
        }
    }

    /// List model ids from the provider's OpenAI-compatible `GET /models` endpoint.
    pub async fn list_models(base_url: &str, api_key: &str) -> AppResult<Vec<String>> {
        #[derive(Deserialize)]
        struct ModelEntry {
            id: String,
        }
        #[derive(Deserialize)]
        struct ModelList {
            data: Vec<ModelEntry>,
        }

        let url = format!("{}/models", base_url.trim_end_matches('/'));
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|e| AppError::Internal(format!("Failed to build HTTP client: {e}")))?;

        let mut request = client.get(&url);
        if !api_key.is_empty() {
            request = request
                .header("Authorization", format!("Bearer {api_key}"))
                // Anthropic's /v1/models expects these instead of a Bearer token
                .header("x-api-key", api_key)
                .header("anthropic-version", "2023-06-01");
        }

        let response = request
            .send()
            .await
            .map_err(|e| AppError::BadRequest(format!("Could not reach {url}: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(AppError::BadRequest(format!(
                "Provider returned {status} for {url}: {}",
                text.chars().take(300).collect::<String>()
            )));
        }

        let list = response
            .json::<ModelList>()
            .await
            .map_err(|e| AppError::BadRequest(format!("Unexpected /models response: {e}")))?;

        let mut ids: Vec<String> = list.data.into_iter().map(|m| m.id).collect();
        ids.sort();
        ids.dedup();
        Ok(ids)
    }

    /// Ollama is recognised by its URL (host name or default port 11434).
    fn detect_api_type(base_url: &str) -> ApiType {
        let url = base_url.to_lowercase();
        if url.contains("ollama") || url.contains(":11434") {
            ApiType::Ollama
        } else {
            ApiType::OpenAI
        }
    }

    pub async fn chat_completion(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<Tool>>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
    ) -> AppResult<ChatCompletionResponse> {
        let url = format!("{}/chat/completions", self.base_url);

        // Build request body manually to handle API-specific argument formatting
        let request_body = self.build_request_body(&messages, &tools, temperature, max_tokens);

        // Create log folder with timestamp and model name
        let timestamp = Local::now().format("%Y%m%d_%H%M%S_%3f").to_string();
        let log_dir = PathBuf::from("logs").join(format!("{}_llm-{}", timestamp, self.model));

        if let Err(e) = fs::create_dir_all(&log_dir) {
            tracing::warn!("Failed to create log directory: {}", e);
        } else {
            // Log request
            let request_json = serde_json::to_string_pretty(&request_body).unwrap_or_default();
            let request_log = format!(
                "URL: {}\nAuthorization: Bearer <REDACTED>\nContent-Type: application/json\nAPI Type: {:?}\n\nRequest Body:\n{}\n",
                url, self.api_type, request_json
            );

            if let Err(e) = fs::write(log_dir.join("request.txt"), &request_log) {
                tracing::warn!("Failed to write request log: {}", e);
            }

            // Generate curl command
            let curl_command = format!(
                "#!/bin/bash\ncurl -X POST '{}' \\\n  -H 'Authorization: Bearer YOUR_API_KEY_HERE' \\\n  -H 'Content-Type: application/json' \\\n  --data-raw '{}'\n",
                url,
                request_json.replace('\'', "'\"'\"'")
            );

            if let Err(e) = fs::write(log_dir.join("curl.sh"), &curl_command) {
                tracing::warn!("Failed to write curl script: {}", e);
            }
        }

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
            .map_err(|e| AppError::Internal(format!("Failed to send request to LLM: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());

            // Log error response
            if let Ok(log_dir_path) = PathBuf::from("logs").join(format!("{}_llm-{}", timestamp, self.model)).canonicalize() {
                let error_log = format!("Status: {}\n\nError:\n{}\n", status, error_text);
                let _ = fs::write(log_dir_path.join("response.txt"), &error_log);
            }

            return Err(AppError::Internal(format!(
                "LLM API error ({status}): {error_text}"
            )));
        }

        let rate_limit = RateLimit::from_headers(response.headers());
        let mut completion = response.json::<ChatCompletionResponse>().await.map_err(|e| {
            AppError::Internal(format!("Failed to parse LLM response: {e}"))
        })?;
        completion.rate_limit = rate_limit;

        // Log successful response
        let response_log = format!(
            "Status: 200 OK\n\nResponse Body:\n{}\n",
            serde_json::to_string_pretty(&completion).unwrap_or_default()
        );

        let _ = fs::write(log_dir.join("response.txt"), &response_log);

        Ok(completion)
    }

    /// Build request body with API-specific argument formatting
    fn build_request_body(
        &self,
        messages: &[Message],
        tools: &Option<Vec<Tool>>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
    ) -> serde_json::Value {
        use serde_json::json;

        let mut request = json!({
            "model": self.model,
        });

        // Build messages with API-specific tool_calls formatting
        let formatted_messages: Vec<serde_json::Value> = messages
            .iter()
            .map(|msg| {
                let mut m = json!({
                    "role": msg.role,
                });

                if let Some(content) = &msg.content {
                    m["content"] = json!(content);
                }

                if let Some(tool_call_id) = &msg.tool_call_id {
                    m["tool_call_id"] = json!(tool_call_id);
                }

                if let Some(tool_calls) = &msg.tool_calls {
                    let formatted_calls: Vec<serde_json::Value> = tool_calls
                        .iter()
                        .map(|tc| {
                            let args = match self.api_type {
                                ApiType::OpenAI => {
                                    // OpenAI expects arguments as a JSON string
                                    json!(serde_json::to_string(&tc.function.arguments).unwrap_or_default())
                                }
                                ApiType::Ollama => {
                                    // Ollama expects arguments as a JSON object
                                    tc.function.arguments.clone()
                                }
                            };

                            json!({
                                "id": tc.id,
                                "type": tc.r#type,
                                "function": {
                                    "name": tc.function.name,
                                    "arguments": args
                                }
                            })
                        })
                        .collect();

                    m["tool_calls"] = json!(formatted_calls);
                }

                m
            })
            .collect();

        request["messages"] = json!(formatted_messages);

        if let Some(t) = tools {
            request["tools"] = serde_json::to_value(t).unwrap_or_default();
        }

        if let Some(temp) = temperature {
            request["temperature"] = json!(temp);
        }

        if let Some(max) = max_tokens {
            request["max_tokens"] = json!(max);
        }

        request
    }

    /// Convert MCP tools to OpenAI tool format
    pub fn convert_mcp_tools_to_openai(mcp_tools: &[rmcp::model::Tool]) -> Vec<Tool> {
        mcp_tools
            .iter()
            .map(|tool| Tool {
                r#type: "function".to_string(),
                function: ToolFunction {
                    name: tool.name.to_string(),
                    description: tool.description.clone().unwrap_or_default().to_string(),
                    parameters: serde_json::Value::Object((*tool.input_schema).clone()),
                },
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_type_comes_from_the_url_not_the_model() {
        // Open-weight model names behind OpenAI-compatible gateways
        for (url, model) in [
            ("https://llm.example.com/v1", "qwen3.8-27b-stable"),
            ("https://api.groq.com/openai/v1", "llama-3.3-70b-versatile"),
            ("https://api.mistral.ai/v1", "mistral-large-latest"),
        ] {
            let client = LlmClient::new(url.to_string(), "k".to_string(), model.to_string());
            assert_eq!(client.api_type, ApiType::OpenAI, "{url} {model}");
        }
        for url in ["http://localhost:11434/v1", "http://ollama.internal/v1"] {
            let client = LlmClient::new(url.to_string(), "k".to_string(), "qwen3".to_string());
            assert_eq!(client.api_type, ApiType::Ollama, "{url}");
        }
    }

    #[test]
    fn test_tool_call_arguments_are_a_string_for_openai_compatible_apis() {
        let client = LlmClient::new("https://llm.example.com/v1".to_string(), "k".to_string(), "qwen3.8-27b-stable".to_string());
        let messages = vec![Message {
            role: "assistant".to_string(),
            content: None,
            tool_calls: Some(vec![ToolCall {
                id: "call_1".to_string(),
                r#type: "function".to_string(),
                function: FunctionCall {
                    name: "search".to_string(),
                    arguments: serde_json::json!({"count": 10, "query": "Model Context Protocol"}),
                },
            }]),
            tool_call_id: None,
        }];
        let body = client.build_request_body(&messages, &None, None, None);
        let arguments = &body["messages"][0]["tool_calls"][0]["function"]["arguments"];
        assert!(arguments.is_string(), "{body}");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(arguments.as_str().unwrap()).unwrap(),
            serde_json::json!({"count": 10, "query": "Model Context Protocol"})
        );
    }

    #[test]
    fn test_request_preview_keeps_only_new_messages_and_tool_names() {
        let msg = |role: &str, content: &str| Message { role: role.into(), content: Some(content.into()), tool_calls: None, tool_call_id: None };
        let messages = vec![msg("user", "hi"), msg("assistant", "hello"), msg("tool", &"x".repeat(2500))];
        let tools = Some(vec![Tool { r#type: "function".into(), function: ToolFunction { name: "echo".into(), description: String::new(), parameters: serde_json::json!({}) } }]);
        let preview = request_preview("m", &messages, &tools, 2);
        assert_eq!(preview["message_count"], 3);
        assert_eq!(preview["new_messages"].as_array().unwrap().len(), 1);
        assert!(preview["new_messages"][0]["content"].as_str().unwrap().ends_with("… [500 more characters]"));
        assert_eq!(preview["tools"], serde_json::json!(["echo"]));
    }

    #[test]
    fn test_cap_tool_result() {
        assert_eq!(cap_tool_result("short", Some(10)), "short");
        assert_eq!(cap_tool_result("no cap at all", None), "no cap at all");
        let capped = cap_tool_result("abcdefghijkl", Some(2));
        assert!(capped.starts_with("abcdefgh\n\n[Tool result truncated to about 2 tokens: 8 of 12 characters shown.]"), "{capped}");
        // Never splits a multi-byte character
        assert!(cap_tool_result("äöüßäöüßäöü", Some(2)).starts_with("äöüßäöüß\n"));
    }

    #[test]
    fn test_rate_limit_from_headers() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-ratelimit-limit-tokens", "8000".parse().unwrap());
        headers.insert("x-ratelimit-remaining-tokens", "514".parse().unwrap());
        assert_eq!(RateLimit::from_headers(&headers), RateLimit { limit_tokens: Some(8000), remaining_tokens: Some(514) });

        let mut anthropic = reqwest::header::HeaderMap::new();
        anthropic.insert("anthropic-ratelimit-tokens-limit", "40000".parse().unwrap());
        assert_eq!(RateLimit::from_headers(&anthropic).limit_tokens, Some(40000));
        assert_eq!(RateLimit::from_headers(&reqwest::header::HeaderMap::new()), RateLimit::default());
    }

    #[test]
    fn test_tool_result_text_joins_text_items() {
        let result = serde_json::json!({
            "content": [{"type": "text", "text": "a"}, {"type": "image", "data": "x"}, {"type": "text", "text": "b"}],
            "isError": false
        });
        assert_eq!(tool_result_text(&result), "a\nb");
    }

    #[test]
    fn test_tool_result_text_falls_back_to_json() {
        let result = serde_json::json!({"structuredContent": {"n": 1}});
        assert_eq!(tool_result_text(&result), r#"{"structuredContent":{"n":1}}"#);
    }

    #[test]
    fn test_client_creation() {
        let client = LlmClient::new(
            "https://api.openai.com/v1".to_string(),
            "test-key".to_string(),
            "gpt-4".to_string(),
        );
        assert_eq!(client.base_url, "https://api.openai.com/v1");
    }

    #[test]
    fn test_base_url_trimming() {
        let client = LlmClient::new(
            "https://api.openai.com/v1/".to_string(),
            "test-key".to_string(),
            "gpt-4".to_string(),
        );
        assert_eq!(client.base_url, "https://api.openai.com/v1");
    }
}
