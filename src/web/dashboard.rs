//! Dashboard: counts for providers, servers and conversations, and a
//! getting-started checklist driven by those counts.

use askama::Template;
use axum::{extract::State, response::Html, Extension};

use super::{render, Layout, WebResult};
use crate::{middleware::auth::AuthUser, AppState};

pub struct DashCard {
    pub href: &'static str,
    pub title: &'static str,
    pub text: &'static str,
    pub count: i64,
    pub unit: &'static str,
    pub action: &'static str,
    pub icon: &'static str,
}

pub struct Step {
    pub href: &'static str,
    pub text: &'static str,
    pub done: bool,
}

#[derive(Template)]
#[template(path = "dashboard.html")]
struct DashboardPage {
    layout: Layout,
    cards: Vec<DashCard>,
    steps: Vec<Step>,
    steps_done: usize,
}

const ROBOT: &str = r#"<path d="M12 2a2 2 0 012 2c0 .74-.4 1.39-1 1.73V7h1a7 7 0 017 7h1a1 1 0 110 2h-1a7 7 0 01-7 7H9a7 7 0 01-7-7H1a1 1 0 110-2h1a7 7 0 017-7h1V5.73c-.6-.34-1-.99-1-1.73a2 2 0 012-2zm-1 9a1 1 0 00-1 1v1a1 1 0 002 0v-1a1 1 0 00-1-1zm4 0a1 1 0 00-1 1v1a1 1 0 002 0v-1a1 1 0 00-1-1z"/>"#;
const SERVER: &str = r#"<path fill-rule="evenodd" d="M4 4a2 2 0 0 0-2 2v3a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2V6a2 2 0 0 0-2-2H4Zm2 3.5a1 1 0 1 0 0-2 1 1 0 0 0 0 2Zm0 9a1 1 0 1 0 0-2 1 1 0 0 0 0 2ZM4 13a2 2 0 0 0-2 2v3a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-3a2 2 0 0 0-2-2H4Z" clip-rule="evenodd"/>"#;
const CHAT: &str = r#"<path d="M4.913 2.658c2.075-.27 4.19-.408 6.337-.408 2.147 0 4.262.139 6.337.408 1.922.25 3.291 1.861 3.405 3.727a4.403 4.403 0 0 0-1.032-.211 50.89 50.89 0 0 0-8.42 0c-2.358.196-4.04 2.19-4.04 4.434v4.286a4.47 4.47 0 0 0 2.433 3.984L7.28 21.53A.75.75 0 0 1 6 21v-4.03a48.527 48.527 0 0 1-1.087-.128C2.905 16.58 1.5 14.833 1.5 12.862V6.638c0-1.97 1.405-3.718 3.413-3.979Z"/><path d="M15.75 7.5c-1.376 0-2.739.057-4.086.169C10.124 7.797 9 9.103 9 10.609v4.285c0 1.507 1.128 2.814 2.67 2.94 1.243.102 2.5.157 3.768.165l2.782 2.781a.75.75 0 0 0 1.28-.53v-2.39l.33-.026c1.542-.125 2.67-1.433 2.67-2.94v-4.286c0-1.505-1.125-2.811-2.664-2.94A49.392 49.392 0 0 0 15.75 7.5Z"/>"#;

async fn count(state: &AppState, table: &str, user_id: i64) -> WebResult<i64> {
    // table is one of three fixed names below, never user input
    Ok(sqlx::query_scalar::<_, i64>(&format!("SELECT COUNT(*) FROM {table} WHERE user_id = ?"))
        .bind(user_id)
        .fetch_one(&state.db)
        .await?)
}

fn plural(n: i64, one: &'static str, many: &'static str) -> &'static str {
    if n == 1 {
        one
    } else {
        many
    }
}

pub async fn page(State(state): State<AppState>, Extension(user): Extension<AuthUser>) -> WebResult<Html<String>> {
    let providers = count(&state, "llm_providers", user.user_id).await?;
    let servers = count(&state, "mcp_servers", user.user_id).await?;
    let conversations = count(&state, "conversations", user.user_id).await?;

    let cards = vec![
        DashCard {
            href: "/llm-providers",
            title: "LLM Providers",
            text: "Manage your AI model providers and API keys",
            count: providers,
            unit: plural(providers, "provider configured", "providers configured"),
            action: "Configure",
            icon: ROBOT,
        },
        DashCard {
            href: "/mcp-servers",
            title: "MCP Servers",
            text: "Connect to local and remote MCP servers",
            count: servers,
            unit: plural(servers, "server", "servers"),
            action: "Manage",
            icon: SERVER,
        },
        DashCard {
            href: "/chat",
            title: "Chat",
            text: "Start conversations with AI assistants",
            count: conversations,
            unit: plural(conversations, "conversation", "conversations"),
            action: "Open",
            icon: CHAT,
        },
    ];
    let steps = vec![
        Step { href: "/llm-providers", text: "Configure an LLM provider with your API key", done: providers > 0 },
        Step { href: "/mcp-servers", text: "Connect to MCP servers for additional tools and context", done: servers > 0 },
        Step { href: "/chat", text: "Start chatting with AI using your configured providers", done: conversations > 0 },
    ];
    let steps_done = steps.iter().filter(|s| s.done).count();

    render(DashboardPage { layout: Layout::for_user(&state, &user, "/dashboard").await?, cards, steps, steps_done })
}
