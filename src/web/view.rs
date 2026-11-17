//! View models shared by several pages: MCP tool results and the timeline of
//! LLM rounds (tools page LLM test, chat).

use serde_json::Value;

use super::format_number;
use crate::models::conversation::LlmStep;

pub struct ResultItem {
    pub text: Option<String>,
    /// data: URI of an image item (only image/* types with plain base64)
    pub image_src: Option<String>,
}

/// An MCP CallToolResult for display
pub struct ToolResultView {
    pub failed: bool,
    pub error: Option<String>,
    pub items: Vec<ResultItem>,
    /// Pretty JSON when the result has no text or image items, or has other kinds
    pub raw_json: Option<String>,
}

fn safe_image_src(mime: &str, data: &str) -> Option<String> {
    let mime_ok = mime.strip_prefix("image/").is_some_and(|t| {
        !t.is_empty() && t.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
    });
    let data_ok = !data.is_empty() && data.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '='));
    (mime_ok && data_ok).then(|| format!("data:{mime};base64,{data}"))
}

impl ToolResultView {
    pub fn error(message: String) -> Self {
        ToolResultView { failed: true, error: Some(message), items: vec![], raw_json: None }
    }

    pub fn from_value(value: &Value) -> Self {
        if let Some(error) = value.get("error").and_then(Value::as_str) {
            return Self::error(error.to_string());
        }
        let mut items = Vec::new();
        let mut other = false;
        for item in value["content"].as_array().into_iter().flatten() {
            match item["type"].as_str() {
                Some("text") => items.push(ResultItem { text: item["text"].as_str().map(str::to_string), image_src: None }),
                Some("image") => match safe_image_src(item["mimeType"].as_str().unwrap_or("image/png"), item["data"].as_str().unwrap_or("")) {
                    Some(src) => items.push(ResultItem { text: None, image_src: Some(src) }),
                    None => other = true,
                },
                _ => other = true,
            }
        }
        let raw_json = (items.is_empty() || other).then(|| serde_json::to_string_pretty(value).unwrap_or_default());
        ToolResultView { failed: value["isError"].as_bool().unwrap_or(false), error: None, items, raw_json }
    }
}

pub struct ToolCallView {
    pub name: String,
    pub args_json: String,
    pub done: bool,
    pub result: ToolResultView,
}

impl ToolCallView {
    pub fn new(name: &str, arguments: &Value, result: Option<&Value>) -> Self {
        ToolCallView {
            name: name.to_string(),
            args_json: serde_json::to_string_pretty(arguments).unwrap_or_default(),
            done: result.is_some(),
            result: result.map(ToolResultView::from_value).unwrap_or_else(|| ToolResultView {
                failed: false,
                error: None,
                items: vec![],
                raw_json: None,
            }),
        }
    }
}

/// One LLM round, followed by the tool calls it triggered
pub struct StepView {
    pub round: u32,
    pub summary: String,
    pub is_error: bool,
    pub meta: String,
    /// Text the LLM wrote alongside its tool calls (otherwise easy to miss)
    pub intermediate: Option<String>,
    pub sent_header: String,
    pub sent_json: String,
    pub received_header: String,
    pub received_json: Option<String>,
    pub error: Option<String>,
    pub tools: Vec<ToolCallView>,
}

fn duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms} ms")
    } else {
        format!("{:.1} s", ms as f64 / 1000.0)
    }
}

/// Steps with their tool calls attached; calls no step points to go to the last step
pub fn steps(llm_steps: &[LlmStep], calls: Vec<ToolCallView>) -> Vec<StepView> {
    let mut calls: Vec<Option<ToolCallView>> = calls.into_iter().map(Some).collect();
    let mut views: Vec<StepView> = llm_steps
        .iter()
        .map(|step| {
            let response = step.response.as_ref();
            let tool_names: Vec<&str> = response
                .and_then(|r| r["tool_calls"].as_array())
                .map(|a| a.iter().filter_map(|c| c["name"].as_str()).collect())
                .unwrap_or_default();
            let summary = if step.error.is_some() {
                "failed".to_string()
            } else if !tool_names.is_empty() {
                format!("asked for {}", tool_names.join(", "))
            } else {
                "answered".to_string()
            };
            let mut meta = duration(step.duration_ms);
            if let Some(usage) = &step.usage {
                if let (Some(prompt), completion) = (usage["prompt_tokens"].as_i64(), usage["completion_tokens"].as_i64().unwrap_or(0)) {
                    meta.push_str(&format!(" · {} → {} tokens", format_number(prompt), format_number(completion)));
                }
            }
            let content = response.and_then(|r| r["content"].as_str()).filter(|c| !c.is_empty());
            let request = &step.request;
            StepView {
                round: step.round,
                summary,
                is_error: step.error.is_some(),
                meta,
                intermediate: content.filter(|_| !tool_names.is_empty()).map(str::to_string),
                sent_header: format!(
                    "Sent: {} messages to {}, {} tools offered. New in this round:",
                    request["message_count"].as_u64().unwrap_or(0),
                    request["model"].as_str().unwrap_or("?"),
                    request["tools"].as_array().map(|t| t.len()).unwrap_or(0)
                ),
                sent_json: serde_json::to_string_pretty(&request["new_messages"]).unwrap_or_default(),
                received_header: format!(
                    "Received (finish reason: {})",
                    response.and_then(|r| r["finish_reason"].as_str()).unwrap_or("none")
                ),
                received_json: response.map(|r| serde_json::to_string_pretty(r).unwrap_or_default()),
                error: step.error.clone(),
                tools: step.tool_call_indices.iter().filter_map(|i| calls.get_mut(*i).and_then(Option::take)).collect(),
            }
        })
        .collect();
    let leftover: Vec<ToolCallView> = calls.into_iter().flatten().collect();
    if let Some(last) = views.last_mut() {
        last.tools.extend(leftover);
    }
    views
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_results_show_text_and_safe_images_only() {
        let view = ToolResultView::from_value(&json!({ "content": [
            { "type": "text", "text": "hi" },
            { "type": "image", "mimeType": "image/png", "data": "iVBORw0KGgo=" },
            { "type": "image", "mimeType": "text/html", "data": "PHNjcmlwdD4=" }
        ], "isError": false }));
        assert_eq!(view.items.len(), 2);
        assert_eq!(view.items[1].image_src.as_deref(), Some("data:image/png;base64,iVBORw0KGgo="));
        assert!(view.raw_json.is_some(), "unknown/unsafe items fall back to raw JSON");
    }

    #[test]
    fn steps_attach_their_tool_calls() {
        let step = |round, tools: Vec<usize>, response: Value| LlmStep {
            round,
            request: json!({ "model": "m", "message_count": 1, "new_messages": [], "tools": ["echo"] }),
            response: Some(response),
            usage: Some(json!({ "prompt_tokens": 1200, "completion_tokens": 5 })),
            duration_ms: 1500,
            error: None,
            tool_call_indices: tools,
        };
        let views = steps(
            &[
                step(1, vec![0], json!({ "content": "Let me check.", "tool_calls": [{ "name": "echo" }] })),
                step(2, vec![], json!({ "content": "Done", "tool_calls": [] })),
            ],
            vec![ToolCallView::new("echo", &json!({}), Some(&json!({ "content": [] })))],
        );
        assert_eq!(views[0].summary, "asked for echo");
        assert_eq!(views[0].meta, "1.5 s · 1,200 → 5 tokens");
        assert_eq!(views[0].intermediate.as_deref(), Some("Let me check."));
        assert_eq!(views[0].tools.len(), 1);
        assert_eq!(views[1].summary, "answered");
        assert!(views[1].intermediate.is_none());
    }
}
