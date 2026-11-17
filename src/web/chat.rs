//! Chat pages: conversation list, messages rendered on the server (Markdown
//! to sanitized HTML, LLM rounds timeline) and a small chat.js that talks to
//! the existing WebSocket and swaps in each finished answer from here.

use askama::Template;
use axum::{
    extract::{Path, State},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{delete, get},
    Extension, Form, Json, Router,
};
use serde::Deserialize;

use super::{
    render,
    view::{self, StepView, ToolCallView},
    Layout, WebResult,
};
use crate::{
    middleware::auth::AuthUser,
    models::conversation::{Conversation, CreateConversationRequest, LlmStep, Message, StoredToolCall},
    routes::{
        conversations::{create_conversation, delete_conversation, get_conversation, list_conversations, list_messages},
        llm_providers::list_providers,
    },
    AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/chat", get(chat_home).post(create))
        .route("/chat/:id", get(chat_page))
        .route("/chat/:id/delete", delete(remove))
        .route("/chat/:id/messages/:message_id", get(message_fragment))
}

/// Markdown from the LLM to HTML; ammonia removes scripts, event handlers and
/// anything else unsafe, links get rel="noopener noreferrer".
pub fn markdown_to_html(markdown: &str) -> String {
    use pulldown_cmark::{html, Options, Parser};
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut out = String::new();
    html::push_html(&mut out, Parser::new_ext(markdown, options));
    ammonia::clean(&out)
}

pub struct ConversationItem {
    pub id: i64,
    pub title: String,
    pub date: String,
    pub active: bool,
}

pub struct MessageView {
    pub id: i64,
    pub is_user: bool,
    pub text: String,
    pub html: String,
    pub time: String,
    /// RFC 3339 timestamp; app.js shows it in the viewer's local time
    pub iso: String,
    /// LLM rounds with their tool calls (answers from before the rounds were saved have none)
    pub steps: Vec<StepView>,
    /// Tool calls of older answers without saved rounds
    pub tools: Vec<ToolCallView>,
}

impl From<Message> for MessageView {
    fn from(m: Message) -> Self {
        let calls: Vec<ToolCallView> = m
            .tool_calls
            .as_deref()
            .and_then(|j| serde_json::from_str::<Vec<StoredToolCall>>(j).ok())
            .unwrap_or_default()
            .iter()
            .map(|c| ToolCallView::new(&c.tool_name, &c.arguments, c.result.as_ref()))
            .collect();
        let llm_steps: Vec<LlmStep> =
            m.llm_steps.as_deref().and_then(|j| serde_json::from_str(j).ok()).unwrap_or_default();
        let is_user = m.role == "user";
        // A single round without tool calls adds nothing worth showing
        let show_steps = llm_steps.len() > 1 || !calls.is_empty();
        let (steps, tools) = if llm_steps.is_empty() {
            (Vec::new(), calls)
        } else if show_steps {
            (view::steps(&llm_steps, calls), Vec::new())
        } else {
            (Vec::new(), Vec::new())
        };
        MessageView {
            id: m.id,
            is_user,
            html: if is_user { String::new() } else { markdown_to_html(&m.content) },
            text: m.content,
            time: m.created_at.format("%H:%M UTC").to_string(),
            iso: m.created_at.to_rfc3339(),
            steps,
            tools,
        }
    }
}

pub struct ProviderChoice {
    pub id: i64,
    pub label: String,
    pub selected: bool,
}

#[derive(Template)]
#[template(path = "chat.html")]
struct ChatPage {
    layout: Layout,
    conversations: Vec<ConversationItem>,
    current: Option<Conversation>,
    messages: Vec<MessageView>,
    providers: Vec<ProviderChoice>,
}

#[derive(Template)]
#[template(path = "partials/message.html")]
struct MessageFragment {
    msg: MessageView,
}

async fn page_data(state: &AppState, user: &AuthUser, current: Option<Conversation>) -> WebResult<ChatPage> {
    let Json(conversations) = list_conversations(State(state.clone()), Extension(user.clone())).await?;
    let current_id = current.as_ref().map(|c| c.id);
    let messages = match current_id {
        Some(id) => {
            let Json(list) = list_messages(State(state.clone()), Extension(user.clone()), Path(id)).await?;
            list.into_iter().map(MessageView::from).collect()
        }
        None => Vec::new(),
    };
    let Json(providers) = list_providers(State(state.clone()), Extension(user.clone())).await?;
    Ok(ChatPage {
        layout: Layout::for_user(state, user, "/chat").await?,
        conversations: conversations
            .into_iter()
            .map(|c| ConversationItem {
                active: Some(c.id) == current_id,
                id: c.id,
                title: c.title,
                date: c.updated_at.format("%Y-%m-%d").to_string(),
            })
            .collect(),
        current,
        messages,
        providers: providers
            .into_iter()
            .map(|p| ProviderChoice { id: p.id, label: format!("{} · {}", p.name, p.model.unwrap_or_default()), selected: p.is_default })
            .collect(),
    })
}

/// /chat opens the most recent conversation, or the empty state
async fn chat_home(State(state): State<AppState>, Extension(user): Extension<AuthUser>) -> WebResult<Response> {
    let Json(conversations) = list_conversations(State(state.clone()), Extension(user.clone())).await?;
    if let Some(latest) = conversations.first() {
        return Ok(Redirect::to(&format!("/chat/{}", latest.id)).into_response());
    }
    Ok(render(page_data(&state, &user, None).await?)?.into_response())
}

async fn chat_page(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> WebResult<Html<String>> {
    let Json(conversation) = get_conversation(State(state.clone()), Extension(user.clone()), Path(id)).await?;
    render(page_data(&state, &user, Some(conversation)).await?)
}

#[derive(Deserialize)]
struct NewChatForm {
    #[serde(default)]
    title: String,
}

async fn create(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    headers: axum::http::HeaderMap,
    Form(form): Form<NewChatForm>,
) -> WebResult<Response> {
    let title = if form.title.trim().is_empty() { "New chat".to_string() } else { form.title.trim().to_string() };
    let (_, Json(conversation)) =
        create_conversation(State(state), Extension(user), Json(CreateConversationRequest { title })).await?;
    let target = format!("/chat/{}", conversation.id);
    // htmx follows a 303 silently and would lose HX-Redirect, so answer htmx with 200
    if headers.contains_key("hx-request") {
        Ok(([("HX-Redirect", target)], "").into_response())
    } else {
        Ok(Redirect::to(&target).into_response())
    }
}

async fn remove(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> WebResult<Response> {
    delete_conversation(State(state), Extension(user), Path(id)).await?;
    Ok(([("HX-Redirect", "/chat")], "").into_response())
}

async fn message_fragment(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path((id, message_id)): Path<(i64, i64)>,
) -> WebResult<Html<String>> {
    // list_messages checks that the conversation belongs to the user
    let Json(list) = list_messages(State(state), Extension(user), Path(id)).await?;
    let message = list
        .into_iter()
        .find(|m| m.id == message_id)
        .ok_or_else(|| crate::error::AppError::NotFound("Message not found".into()))?;
    render(MessageFragment { msg: message.into() })
}

#[cfg(test)]
mod tests {
    use super::markdown_to_html;

    #[test]
    fn renders_markdown_and_strips_unsafe_html() {
        let html = markdown_to_html("**bold** <script>alert(1)</script> [x](javascript:alert(1)) <img src=x onerror=alert(1)>");
        assert!(html.contains("<strong>bold</strong>"), "{html}");
        assert!(!html.contains("<script"), "{html}");
        assert!(!html.contains("javascript:"), "{html}");
        assert!(!html.contains("onerror"), "{html}");
    }

    #[test]
    fn renders_tables_and_code() {
        let html = markdown_to_html("| a | b |\n|---|---|\n| 1 | 2 |\n\n```rust\nfn main() {}\n```");
        assert!(html.contains("<table>") && html.contains("<code"), "{html}");
    }
}
