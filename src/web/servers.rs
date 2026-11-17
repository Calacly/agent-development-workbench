//! MCP Servers pages: list with connect/disconnect, add/edit/import dialog,
//! and the tools, resources and prompts pages of a server. Handlers call the
//! JSON API handlers in routes::mcp_servers and render HTML.

use askama::Template;
use axum::{
    extract::{Multipart, Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Extension, Form, Json, Router,
};
use serde::Deserialize;
use serde_json::Value;

use super::{
    mcp_import::{parse_mcp_config, ImportedServer},
    render, user_message,
    view::{self, ToolResultView},
    Layout, WebResult,
};
use crate::{
    middleware::auth::AuthUser,
    models::mcp_server::{CreateMcpServerRequest, McpServerResponse, McpServerStatus, McpServerType, UpdateMcpServerRequest},
    routes::{
        llm_providers::list_providers,
        mcp_servers::{
            call_tool, connect_server, create_server, delete_server, disconnect_server, get_capabilities, get_prompt,
            get_server, list_prompts, list_resources, list_servers, list_tools, read_resource, test_tools_with_llm,
            update_server, CallToolRequest, GetPromptRequest, ReadResourceRequest, TestToolsRequest,
        },
    },
    AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/mcp-servers", get(page).post(create))
        .route("/mcp-servers/new", get(new_form))
        .route("/mcp-servers/import", post(import_file))
        .route("/mcp-servers/import/selected", post(import_selected))
        .route("/mcp-servers/:id", axum::routing::put(update).delete(delete))
        .route("/mcp-servers/:id/edit", get(edit_form))
        .route("/mcp-servers/:id/connect", post(connect))
        .route("/mcp-servers/:id/disconnect", post(disconnect))
        .route("/mcp-servers/:id/tools", get(tools_page))
        .route("/mcp-servers/:id/tools-test", post(tools_test))
        .route("/mcp-servers/:id/tools/:name", get(tool_detail).post(run_tool))
        .route("/mcp-servers/:id/resources", get(resources_page).post(read_resource_fragment))
        .route("/mcp-servers/:id/prompts", get(prompts_page))
        .route("/mcp-servers/:id/prompts/:name", get(prompt_detail).post(run_prompt))
}

// ---------- View models ----------

pub struct Capability {
    pub key: &'static str,
    pub label: &'static str,
    pub supported: bool,
}

pub struct ServerView {
    pub id: i64,
    pub name: String,
    pub type_label: &'static str,
    pub status: &'static str,
    pub status_label: &'static str,
    pub target: String,
    pub use_container: bool,
    pub created: String,
    pub capabilities: Vec<Capability>,
    pub error: Option<String>,
}

fn status_of(status: &McpServerStatus) -> (&'static str, &'static str) {
    match status {
        McpServerStatus::Running => ("running", "Running"),
        McpServerStatus::Error => ("error", "Error"),
        _ => ("stopped", "Stopped"),
    }
}

fn target_of(server_type: &McpServerType, config: &Value) -> String {
    match server_type {
        McpServerType::Http => config["url"].as_str().unwrap_or_default().to_string(),
        _ => {
            let args: Vec<&str> = config["args"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            format!("{} {}", config["command"].as_str().unwrap_or_default(), args.join(" ")).trim().to_string()
        }
    }
}

impl ServerView {
    fn from_response(s: McpServerResponse) -> Self {
        let (status, status_label) = status_of(&s.status);
        ServerView {
            id: s.id,
            name: s.name,
            type_label: if matches!(s.server_type, McpServerType::Http) { "HTTP" } else { "Local (stdio)" },
            status,
            status_label,
            target: target_of(&s.server_type, &s.config),
            use_container: s.use_container,
            created: s.created_at.format("%Y-%m-%d").to_string(),
            capabilities: Vec::new(),
            error: None,
        }
    }
}

async fn server_view(state: &AppState, user: &AuthUser, server: McpServerResponse) -> ServerView {
    let running = matches!(server.status, McpServerStatus::Running);
    let id = server.id;
    let mut view = ServerView::from_response(server);
    if running {
        // Unknown capabilities: offer all three, as before
        let caps = get_capabilities(State(state.clone()), Extension(user.clone()), Path(id))
            .await
            .map(|Json(v)| v)
            .unwrap_or_else(|_| serde_json::json!({ "tools": true, "resources": true, "prompts": true }));
        view.capabilities = [("tools", "Tools"), ("resources", "Resources"), ("prompts", "Prompts")]
            .into_iter()
            .map(|(key, label)| Capability { key, label, supported: caps[key].as_bool().unwrap_or(true) })
            .collect();
    }
    view
}

async fn servers(state: &AppState, user: &AuthUser) -> WebResult<Vec<ServerView>> {
    let Json(list) = list_servers(State(state.clone()), Extension(user.clone())).await?;
    let mut views = Vec::with_capacity(list.len());
    for server in list {
        views.push(server_view(state, user, server).await);
    }
    Ok(views)
}

pub struct Example {
    pub key: &'static str,
    pub name: &'static str,
    pub command: &'static str,
    pub args: &'static str,
}

const EXAMPLES: &[Example] = &[
    Example { key: "git", name: "Git", command: "uvx", args: "mcp-server-git" },
    Example { key: "filesystem", name: "Filesystem", command: "uvx", args: "mcp-server-filesystem /path/to/dir" },
    Example { key: "brave", name: "Brave Search", command: "uvx", args: "mcp-server-brave-search" },
];

pub struct ImagePreset {
    pub key: &'static str,
    pub label: &'static str,
    pub image: &'static str,
}

/// Same images as the backend defaults (Debian slim, see docs/sandbox.md)
const IMAGE_PRESETS: &[ImagePreset] = &[
    ImagePreset { key: "python-uv", label: "Python 3.15 with uv/uvx (recommended)", image: "ghcr.io/astral-sh/uv:python3.15-trixie-slim" },
    ImagePreset { key: "python-slim", label: "Python 3.15 Slim", image: "python:3.15-slim" },
    ImagePreset { key: "node", label: "Node.js 26 Slim", image: "node:26-slim" },
];

/// Values of the add/edit form
#[derive(Default)]
pub struct ServerFormView {
    pub edit_id: Option<i64>,
    pub name: String,
    pub server_type: String,
    pub command: String,
    pub args: String,
    /// Exact imported args (JSON), used while the Arguments field is unchanged
    pub original_args: String,
    pub env: String,
    pub url: String,
    pub use_container: bool,
    pub image_preset: String,
    pub image: String,
    pub container_params: String,
    pub notice: String,
    pub error: String,
}

impl ServerFormView {
    fn new_server() -> Self {
        ServerFormView {
            server_type: "stdio".into(),
            command: "uvx".into(),
            args: "mcp-server-git".into(),
            image_preset: "python-uv".into(),
            image: IMAGE_PRESETS[0].image.into(),
            ..Default::default()
        }
    }

    fn fill_from(&mut self, server_type: &str, config: &Value) {
        self.server_type = server_type.to_string();
        if server_type == "http" {
            self.url = config["url"].as_str().unwrap_or_default().to_string();
        } else {
            self.command = config["command"].as_str().unwrap_or_default().to_string();
            let args: Vec<String> =
                config["args"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
            self.args = args.join(" ");
            self.original_args = serde_json::to_string(&args).unwrap_or_default();
            self.env = config["env"]
                .as_object()
                .filter(|e| !e.is_empty())
                .map(|e| serde_json::to_string_pretty(e).unwrap_or_default())
                .unwrap_or_default();
        }
    }

    fn set_image(&mut self, image: Option<String>) {
        let image = image.unwrap_or_else(|| IMAGE_PRESETS[0].image.to_string());
        self.image_preset = IMAGE_PRESETS.iter().find(|p| p.image == image).map(|p| p.key).unwrap_or("custom").to_string();
        self.image = image;
    }
}

// ---------- Templates ----------

#[derive(Template)]
#[template(path = "mcp_servers.html")]
struct ServersPage {
    layout: Layout,
    servers: Vec<ServerView>,
    examples: &'static [Example],
    notice: String,
}

#[derive(Template)]
#[template(path = "partials/server_list.html")]
struct ServerList {
    servers: Vec<ServerView>,
    notice: String,
}

#[derive(Template)]
#[template(path = "partials/server_card.html")]
struct ServerCard {
    s: ServerView,
}

#[derive(Template)]
#[template(path = "partials/server_form.html")]
struct ServerForm {
    f: ServerFormView,
    presets: &'static [ImagePreset],
}

pub struct ImportChoice {
    pub index: usize,
    pub name: String,
    pub target: String,
    pub disabled: bool,
    pub exists: bool,
    pub checked: bool,
}

#[derive(Template)]
#[template(path = "partials/import_choices.html")]
struct ImportChoices {
    choices: Vec<ImportChoice>,
    servers_json: String,
    message: String,
}

// ---------- List page ----------

async fn page(State(state): State<AppState>, Extension(user): Extension<AuthUser>) -> WebResult<Html<String>> {
    render(ServersPage {
        layout: Layout::for_user(&state, &user, "/mcp-servers").await?,
        servers: servers(&state, &user).await?,
        examples: EXAMPLES,
        notice: String::new(),
    })
}

async fn list_fragment(state: &AppState, user: &AuthUser, notice: String) -> WebResult<Html<String>> {
    render(ServerList { servers: servers(state, user).await?, notice })
}

fn form_error(message: &str) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        [("HX-Retarget", "#server-form-error"), ("HX-Reswap", "innerHTML")],
        askama::filters::escape(message, askama::filters::Html).map(|s| s.to_string()).unwrap_or_default(),
    )
        .into_response()
}

#[derive(Deserialize)]
struct NewQuery {
    example: Option<String>,
}

async fn new_form(Query(query): Query<NewQuery>) -> WebResult<Html<String>> {
    let mut f = ServerFormView::new_server();
    if let Some(example) = query.example.and_then(|key| EXAMPLES.iter().find(|e| e.key == key)) {
        f.name = example.name.into();
        f.command = example.command.into();
        f.args = example.args.into();
    }
    render(ServerForm { f, presets: IMAGE_PRESETS })
}

async fn edit_form(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> WebResult<Html<String>> {
    let Json(server) = get_server(State(state), Extension(user), Path(id)).await?;
    let mut f = ServerFormView { edit_id: Some(id), name: server.name.clone(), ..ServerFormView::new_server() };
    let server_type = if matches!(server.server_type, McpServerType::Http) { "http" } else { "stdio" };
    f.fill_from(server_type, &server.config);
    f.use_container = server.use_container;
    f.set_image(server.container_image.clone());
    f.container_params = server.container_params.clone().unwrap_or_default();
    if matches!(server.status, McpServerStatus::Running) {
        f.notice = "This server is running. Disconnect and connect it again to apply changes.".into();
    }
    render(ServerForm { f, presets: IMAGE_PRESETS })
}

#[derive(Deserialize)]
struct ServerFormData {
    #[serde(default)]
    name: String,
    #[serde(default)]
    server_type: String,
    #[serde(default)]
    command: String,
    #[serde(default)]
    args: String,
    #[serde(default)]
    original_args: String,
    #[serde(default)]
    env: String,
    #[serde(default)]
    url: String,
    use_container: Option<String>,
    #[serde(default)]
    image_preset: String,
    #[serde(default)]
    image: String,
    #[serde(default)]
    container_params: String,
}

struct ParsedForm {
    name: String,
    server_type: McpServerType,
    config: Value,
    use_container: bool,
    container_image: Option<String>,
    container_params: Option<String>,
}

fn parse_form(form: ServerFormData) -> Result<ParsedForm, String> {
    let name = form.name.trim().to_string();
    if name.is_empty() {
        return Err("Enter a server name.".into());
    }
    let (server_type, config) = if form.server_type == "http" {
        let url = form.url.trim();
        if url.is_empty() {
            return Err("Enter the server URL.".into());
        }
        (McpServerType::Http, serde_json::json!({ "url": url }))
    } else {
        let command = form.command.trim();
        if command.is_empty() {
            return Err("Enter the command that starts the server.".into());
        }
        // Imported args keep their exact form (spaces inside arguments) while unchanged
        let original: Option<Vec<String>> = serde_json::from_str(&form.original_args).ok();
        let args: Vec<String> = match original {
            Some(original) if original.join(" ") == form.args => original,
            _ => form.args.split_whitespace().map(str::to_string).collect(),
        };
        let env: Value = if form.env.trim().is_empty() {
            serde_json::json!({})
        } else {
            let value: Value =
                serde_json::from_str(&form.env).map_err(|e| format!("Environment variables must be a JSON object: {e}"))?;
            if !value.is_object() {
                return Err("Environment variables must be a JSON object like {\"KEY\": \"value\"}.".into());
            }
            value
        };
        (McpServerType::Stdio, serde_json::json!({ "command": command, "args": args, "env": env }))
    };
    let use_container = form.use_container.is_some() && matches!(server_type, McpServerType::Stdio);
    let image = IMAGE_PRESETS
        .iter()
        .find(|p| p.key == form.image_preset)
        .map(|p| p.image.to_string())
        .unwrap_or_else(|| form.image.trim().to_string());
    Ok(ParsedForm {
        name,
        server_type,
        config,
        use_container,
        container_image: (use_container && !image.is_empty()).then_some(image),
        container_params: (use_container && !form.container_params.trim().is_empty())
            .then(|| form.container_params.trim().to_string()),
    })
}

async fn create(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Form(form): Form<ServerFormData>,
) -> WebResult<Response> {
    let parsed = match parse_form(form) {
        Ok(parsed) => parsed,
        Err(message) => return Ok(form_error(&message)),
    };
    let request = CreateMcpServerRequest {
        name: parsed.name,
        server_type: parsed.server_type,
        config: parsed.config,
        use_container: parsed.use_container,
        container_image: parsed.container_image,
        container_params: parsed.container_params,
    };
    match create_server(State(state.clone()), Extension(user.clone()), Json(request)).await {
        Ok(_) => Ok(list_fragment(&state, &user, String::new()).await?.into_response()),
        Err(e) => Ok(form_error(&user_message(&e))),
    }
}

async fn update(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
    Form(form): Form<ServerFormData>,
) -> WebResult<Response> {
    let parsed = match parse_form(form) {
        Ok(parsed) => parsed,
        Err(message) => return Ok(form_error(&message)),
    };
    let request = UpdateMcpServerRequest {
        name: Some(parsed.name.clone()),
        config: Some(parsed.config),
        use_container: Some(parsed.use_container),
        container_image: parsed.container_image,
        container_params: parsed.container_params,
    };
    match update_server(State(state.clone()), Extension(user.clone()), Path(id), Json(request)).await {
        Ok(Json(server)) => {
            let notice = if matches!(server.status, McpServerStatus::Running) {
                format!("Saved “{}”. Reconnect it to apply the changes.", parsed.name)
            } else {
                format!("Saved “{}”.", parsed.name)
            };
            Ok(list_fragment(&state, &user, notice).await?.into_response())
        }
        Err(e) => Ok(form_error(&user_message(&e))),
    }
}

async fn delete(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> WebResult<Html<String>> {
    delete_server(State(state.clone()), Extension(user.clone()), Path(id)).await?;
    list_fragment(&state, &user, String::new()).await
}

async fn card(state: &AppState, user: &AuthUser, id: i64, error: Option<String>) -> WebResult<Html<String>> {
    let Json(server) = get_server(State(state.clone()), Extension(user.clone()), Path(id)).await?;
    let mut s = server_view(state, user, server).await;
    s.error = error;
    render(ServerCard { s })
}

async fn connect(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> WebResult<Html<String>> {
    let error = connect_server(State(state.clone()), Extension(user.clone()), Path(id)).await.err().map(|e| user_message(&e));
    card(&state, &user, id, error).await
}

async fn disconnect(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> WebResult<Html<String>> {
    let error = disconnect_server(State(state.clone()), Extension(user.clone()), Path(id)).await.err().map(|e| user_message(&e));
    card(&state, &user, id, error).await
}

// ---------- Import ----------

async fn existing_names(state: &AppState, user: &AuthUser) -> WebResult<Vec<String>> {
    let Json(list) = list_servers(State(state.clone()), Extension(user.clone())).await?;
    Ok(list.into_iter().map(|s| s.name).collect())
}

fn form_from_import(server: &ImportedServer, notice: String) -> ServerFormView {
    let mut f = ServerFormView { name: server.name.clone(), notice, ..ServerFormView::new_server() };
    f.fill_from(&server.server_type, &server.config);
    f
}

async fn import_file(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    mut multipart: Multipart,
) -> WebResult<Html<String>> {
    let mut text = None;
    let mut file_name = String::from("the file");
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("file") {
            file_name = field.file_name().unwrap_or("the file").to_string();
            text = field.text().await.ok();
        }
    }
    let error_form = |message: String| {
        let mut f = ServerFormView::new_server();
        f.error = message;
        render(ServerForm { f, presets: IMAGE_PRESETS })
    };
    let Some(text) = text.filter(|t| !t.trim().is_empty()) else {
        return error_form("Choose an mcp.json file to import.".into());
    };
    let result = match parse_mcp_config(&text) {
        Ok(result) => result,
        Err(message) => return error_form(message),
    };
    let skipped = if result.skipped.is_empty() { String::new() } else { format!(" Skipped: {}.", result.skipped.join("; ")) };
    match result.servers.len() {
        0 => error_form(format!("No usable servers in {file_name}.{skipped}")),
        1 => render(ServerForm {
            f: form_from_import(&result.servers[0], format!("Loaded “{}”. Review the fields and click Add Server.{skipped}", result.servers[0].name)),
            presets: IMAGE_PRESETS,
        }),
        n => {
            let existing = existing_names(&state, &user).await?;
            let choices = result
                .servers
                .iter()
                .enumerate()
                .map(|(index, s)| {
                    let exists = existing.contains(&s.name);
                    ImportChoice {
                        index,
                        name: s.name.clone(),
                        target: target_of(
                            if s.server_type == "http" { &McpServerType::Http } else { &McpServerType::Stdio },
                            &s.config,
                        ),
                        disabled: s.disabled,
                        exists,
                        // Preselect what is enabled in the file and not added yet
                        checked: !s.disabled && !exists,
                    }
                })
                .collect();
            render(ImportChoices {
                choices,
                servers_json: serde_json::to_string(&result.servers).unwrap_or_default(),
                message: format!("{n} servers found in {file_name}. Choose which to import.{skipped}"),
            })
        }
    }
}

async fn import_selected(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    body: String,
) -> WebResult<Response> {
    // Form with repeated "pick" fields: parse by hand (serde_urlencoded has no Vec support)
    let mut picks = Vec::new();
    let mut servers_json = String::new();
    for (key, value) in url::form_urlencoded::parse(body.as_bytes()) {
        match key.as_ref() {
            "pick" => picks.extend(value.parse::<usize>().ok()),
            "servers_json" => servers_json = value.into_owned(),
            _ => {}
        }
    }
    let servers: Vec<ImportedServer> = serde_json::from_str(&servers_json).unwrap_or_default();
    let chosen: Vec<&ImportedServer> = picks.iter().filter_map(|i| servers.get(*i)).collect();
    match chosen.len() {
        0 => Ok(form_error("Select at least one server.")),
        // One server: fill the form so it can be configured before adding
        1 => Ok(render(ServerForm {
            f: form_from_import(chosen[0], format!("Loaded “{}”. Review the fields and click Add Server.", chosen[0].name)),
            presets: IMAGE_PRESETS,
        })?
        .into_response()),
        n => {
            let mut failed = Vec::new();
            for server in &chosen {
                let request = CreateMcpServerRequest {
                    name: server.name.clone(),
                    server_type: if server.server_type == "http" { McpServerType::Http } else { McpServerType::Stdio },
                    config: server.config.clone(),
                    use_container: false,
                    container_image: None,
                    container_params: None,
                };
                if let Err(e) = create_server(State(state.clone()), Extension(user.clone()), Json(request)).await {
                    failed.push(format!("{} ({})", server.name, user_message(&e)));
                }
            }
            let notice = if failed.is_empty() {
                format!("Imported {n} servers. Use Edit on a server to adjust it.")
            } else {
                format!("Imported {} of {n}. Failed: {}", n - failed.len(), failed.join(", "))
            };
            // Replace the list (not the dialog body) and close the dialog
            let html = list_fragment(&state, &user, notice).await?;
            Ok(([("HX-Retarget", "#server-list"), ("HX-Reswap", "outerHTML"), ("HX-Trigger", "close-dialog")], html).into_response())
        }
    }
}

// ---------- Server sub pages ----------

pub struct ServerHeader {
    pub id: i64,
    pub name: String,
    pub running: bool,
}

async fn header(state: &AppState, user: &AuthUser, id: i64) -> WebResult<ServerHeader> {
    let Json(server) = get_server(State(state.clone()), Extension(user.clone()), Path(id)).await?;
    Ok(ServerHeader { id, name: server.name, running: matches!(server.status, McpServerStatus::Running) })
}

pub struct ToolItem {
    pub name: String,
    pub description: String,
}

pub struct ProviderOption {
    pub id: i64,
    pub label: String,
    pub selected: bool,
}

#[derive(Template)]
#[template(path = "server_tools.html")]
struct ToolsPage {
    layout: Layout,
    server: ServerHeader,
    tools: Vec<ToolItem>,
    error: Option<String>,
    providers: Vec<ProviderOption>,
}

async fn tools_page(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> WebResult<Html<String>> {
    let server = header(&state, &user, id).await?;
    let (tools, error) = match list_tools(State(state.clone()), Extension(user.clone()), Path(id)).await {
        Ok(Json(value)) => (
            value["tools"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|t| ToolItem {
                            name: t["name"].as_str().unwrap_or_default().to_string(),
                            description: t["description"].as_str().unwrap_or_default().to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            None,
        ),
        Err(e) => (Vec::new(), Some(user_message(&e))),
    };
    let Json(list) = list_providers(State(state.clone()), Extension(user.clone())).await?;
    let providers = list
        .into_iter()
        .map(|p| ProviderOption {
            id: p.id,
            label: format!("{} ({})", p.name, p.model.unwrap_or_default()),
            selected: p.is_default,
        })
        .collect();
    render(ToolsPage { layout: Layout::for_user(&state, &user, "/mcp-servers").await?, server, tools, error, providers })
}

#[derive(Template)]
#[template(path = "partials/tool_detail.html")]
struct ToolDetail {
    server_id: i64,
    name: String,
    description: String,
    schema_json: String,
    required: usize,
    default_args: String,
}

/// Default arguments from the input schema: defaults, else empty values by type
fn default_args(schema: &Value) -> Value {
    let mut args = serde_json::Map::new();
    if let Some(props) = schema["properties"].as_object() {
        for (key, prop) in props {
            let value = if let Some(default) = prop.get("default") {
                default.clone()
            } else {
                match prop["type"].as_str() {
                    Some("string") => Value::String(String::new()),
                    Some("number") | Some("integer") => Value::from(0),
                    Some("boolean") => Value::Bool(false),
                    Some("array") => Value::Array(vec![]),
                    Some("object") => Value::Object(Default::default()),
                    _ => continue,
                }
            };
            args.insert(key.clone(), value);
        }
    }
    Value::Object(args)
}

async fn tool_detail(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path((id, name)): Path<(i64, String)>,
) -> WebResult<Html<String>> {
    let Json(value) = list_tools(State(state), Extension(user), Path(id)).await?;
    let tool = value["tools"]
        .as_array()
        .and_then(|a| a.iter().find(|t| t["name"].as_str() == Some(name.as_str())))
        .cloned()
        .ok_or_else(|| crate::error::AppError::NotFound(format!("Tool {name} not found")))?;
    let schema = tool.get("inputSchema").cloned().unwrap_or(Value::Null);
    render(ToolDetail {
        server_id: id,
        name,
        description: tool["description"].as_str().unwrap_or_default().to_string(),
        schema_json: serde_json::to_string_pretty(&schema).unwrap_or_default(),
        required: schema["required"].as_array().map(|a| a.len()).unwrap_or(0),
        default_args: serde_json::to_string_pretty(&default_args(&schema)).unwrap_or_default(),
    })
}

#[derive(Template)]
#[template(path = "partials/tool_result.html")]
struct ToolResultFragment {
    r: ToolResultView,
}

#[derive(Deserialize)]
struct RunToolForm {
    #[serde(default)]
    arguments: String,
}

async fn run_tool(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path((id, name)): Path<(i64, String)>,
    Form(form): Form<RunToolForm>,
) -> WebResult<Html<String>> {
    let arguments: Value = match serde_json::from_str(if form.arguments.trim().is_empty() { "{}" } else { &form.arguments }) {
        Ok(v) => v,
        Err(e) => {
            return render(ToolResultFragment { r: ToolResultView::error(format!("Arguments are not valid JSON: {e}")) });
        }
    };
    let result = match call_tool(State(state), Extension(user), Path((id, name)), Json(CallToolRequest { arguments })).await {
        Ok(Json(value)) => ToolResultView::from_value(&value),
        Err(e) => ToolResultView::error(user_message(&e)),
    };
    render(ToolResultFragment { r: result })
}

#[derive(Template)]
#[template(path = "partials/llm_test_result.html")]
struct LlmTestResult {
    steps: Vec<view::StepView>,
    response: String,
    error: Option<String>,
    had_tool_calls: bool,
}

#[derive(Deserialize)]
struct LlmTestForm {
    provider_id: i64,
    #[serde(default)]
    prompt: String,
}

async fn tools_test(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
    Form(form): Form<LlmTestForm>,
) -> WebResult<Html<String>> {
    if form.prompt.trim().is_empty() {
        return render(LlmTestResult { steps: vec![], response: String::new(), error: Some("Enter a prompt.".into()), had_tool_calls: false });
    }
    let request = TestToolsRequest { provider_id: form.provider_id, prompt: form.prompt };
    match test_tools_with_llm(State(state), Extension(user), Path(id), Json(request)).await {
        Ok(Json(result)) => {
            let calls: Vec<view::ToolCallView> = result
                .tool_calls
                .iter()
                .map(|c| view::ToolCallView::new(&c.name, &c.arguments, c.result.as_ref()))
                .collect();
            let had_tool_calls = !calls.is_empty();
            render(LlmTestResult {
                steps: view::steps(&result.llm_steps, calls),
                response: result.response,
                error: result.error,
                had_tool_calls,
            })
        }
        Err(e) => render(LlmTestResult { steps: vec![], response: String::new(), error: Some(user_message(&e)), had_tool_calls: false }),
    }
}

// Resources

pub struct ResourceItem {
    pub uri: String,
    pub name: String,
    pub description: String,
    pub mime: String,
}

#[derive(Template)]
#[template(path = "server_resources.html")]
struct ResourcesPage {
    layout: Layout,
    server: ServerHeader,
    resources: Vec<ResourceItem>,
    error: Option<String>,
}

async fn resources_page(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> WebResult<Html<String>> {
    let server = header(&state, &user, id).await?;
    let (resources, error) = match list_resources(State(state.clone()), Extension(user.clone()), Path(id)).await {
        Ok(Json(value)) => (
            value["resources"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|r| ResourceItem {
                            uri: r["uri"].as_str().unwrap_or_default().to_string(),
                            name: r["name"].as_str().unwrap_or_default().to_string(),
                            description: r["description"].as_str().unwrap_or_default().to_string(),
                            mime: r["mimeType"].as_str().unwrap_or_default().to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            None,
        ),
        Err(e) => (Vec::new(), Some(user_message(&e))),
    };
    render(ResourcesPage { layout: Layout::for_user(&state, &user, "/mcp-servers").await?, server, resources, error })
}

pub struct ResourceContent {
    pub uri: String,
    pub mime: String,
    pub text: Option<String>,
    pub blob_size: Option<usize>,
}

#[derive(Template)]
#[template(path = "partials/resource_content.html")]
struct ResourceContentFragment {
    contents: Vec<ResourceContent>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct ReadForm {
    uri: String,
}

async fn read_resource_fragment(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
    Form(form): Form<ReadForm>,
) -> WebResult<Html<String>> {
    match read_resource(State(state), Extension(user), Path(id), Json(ReadResourceRequest { uri: form.uri })).await {
        Ok(Json(value)) => {
            let contents = value["contents"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|c| ResourceContent {
                            uri: c["uri"].as_str().unwrap_or_default().to_string(),
                            mime: c["mimeType"].as_str().unwrap_or_default().to_string(),
                            text: c["text"].as_str().map(str::to_string),
                            blob_size: c["blob"].as_str().map(|b| b.len() * 3 / 4),
                        })
                        .collect()
                })
                .unwrap_or_default();
            render(ResourceContentFragment { contents, error: None })
        }
        Err(e) => render(ResourceContentFragment { contents: vec![], error: Some(user_message(&e)) }),
    }
}

// Prompts

pub struct PromptArg {
    pub name: String,
    pub description: String,
    pub required: bool,
}

pub struct PromptItem {
    pub name: String,
    pub description: String,
    pub args: Vec<PromptArg>,
}

fn prompt_items(value: &Value) -> Vec<PromptItem> {
    value["prompts"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|p| PromptItem {
                    name: p["name"].as_str().unwrap_or_default().to_string(),
                    description: p["description"].as_str().unwrap_or_default().to_string(),
                    args: p["arguments"]
                        .as_array()
                        .map(|args| {
                            args.iter()
                                .map(|arg| PromptArg {
                                    name: arg["name"].as_str().unwrap_or_default().to_string(),
                                    description: arg["description"].as_str().unwrap_or_default().to_string(),
                                    required: arg["required"].as_bool().unwrap_or(false),
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Template)]
#[template(path = "server_prompts.html")]
struct PromptsPage {
    layout: Layout,
    server: ServerHeader,
    prompts: Vec<PromptItem>,
    error: Option<String>,
}

async fn prompts_page(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> WebResult<Html<String>> {
    let server = header(&state, &user, id).await?;
    let (prompts, error) = match list_prompts(State(state.clone()), Extension(user.clone()), Path(id)).await {
        Ok(Json(value)) => (prompt_items(&value), None),
        Err(e) => (Vec::new(), Some(user_message(&e))),
    };
    render(PromptsPage { layout: Layout::for_user(&state, &user, "/mcp-servers").await?, server, prompts, error })
}

#[derive(Template)]
#[template(path = "partials/prompt_detail.html")]
struct PromptDetail {
    server_id: i64,
    p: PromptItem,
}

async fn prompt_detail(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path((id, name)): Path<(i64, String)>,
) -> WebResult<Html<String>> {
    let Json(value) = list_prompts(State(state), Extension(user), Path(id)).await?;
    let p = prompt_items(&value)
        .into_iter()
        .find(|p| p.name == name)
        .ok_or_else(|| crate::error::AppError::NotFound(format!("Prompt {name} not found")))?;
    render(PromptDetail { server_id: id, p })
}

pub struct PromptMessageView {
    pub role: String,
    pub text: String,
}

#[derive(Template)]
#[template(path = "partials/prompt_result.html")]
struct PromptResult {
    description: String,
    messages: Vec<PromptMessageView>,
    error: Option<String>,
}

async fn run_prompt(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path((id, name)): Path<(i64, String)>,
    body: String,
) -> WebResult<Html<String>> {
    let mut args = serde_json::Map::new();
    for (key, value) in url::form_urlencoded::parse(body.as_bytes()) {
        if let Some(arg) = key.strip_prefix("arg_") {
            if !value.is_empty() {
                args.insert(arg.to_string(), Value::String(value.into_owned()));
            }
        }
    }
    let request = GetPromptRequest { arguments: Some(Value::Object(args)) };
    match get_prompt(State(state), Extension(user), Path((id, name)), Json(request)).await {
        Ok(Json(value)) => {
            let messages = value["messages"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|m| PromptMessageView {
                            role: m["role"].as_str().unwrap_or_default().to_string(),
                            text: match m["content"]["type"].as_str() {
                                Some("text") => m["content"]["text"].as_str().unwrap_or_default().to_string(),
                                Some(other) => format!(
                                    "[{other}: {}]",
                                    m["content"]["mimeType"].as_str().unwrap_or("unknown type")
                                ),
                                None => serde_json::to_string_pretty(&m["content"]).unwrap_or_default(),
                            },
                        })
                        .collect()
                })
                .unwrap_or_default();
            render(PromptResult {
                description: value["description"].as_str().unwrap_or_default().to_string(),
                messages,
                error: None,
            })
        }
        Err(e) => render(PromptResult { description: String::new(), messages: vec![], error: Some(user_message(&e)) }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(server_type: &str, args: &str, original: &str, env: &str) -> ServerFormData {
        ServerFormData {
            name: "s".into(),
            server_type: server_type.into(),
            command: "npx".into(),
            args: args.into(),
            original_args: original.into(),
            env: env.into(),
            url: "https://x.example/mcp".into(),
            use_container: None,
            image_preset: "python-uv".into(),
            image: String::new(),
            container_params: String::new(),
        }
    }

    #[test]
    fn keeps_imported_args_while_unchanged() {
        let parsed = parse_form(form("stdio", "a b c", r#"["a b","c"]"#, "")).unwrap();
        assert_eq!(parsed.config["args"], serde_json::json!(["a b", "c"]));
        let edited = parse_form(form("stdio", "a b d", r#"["a b","c"]"#, "")).unwrap();
        assert_eq!(edited.config["args"], serde_json::json!(["a", "b", "d"]));
    }

    #[test]
    fn rejects_env_that_is_not_a_json_object() {
        assert!(parse_form(form("stdio", "x", "", "[1]")).is_err());
        assert!(parse_form(form("stdio", "x", "", "{nope")).is_err());
        assert_eq!(parse_form(form("stdio", "x", "", r#"{"K":"v"}"#)).unwrap().config["env"]["K"], "v");
    }

    #[test]
    fn http_servers_take_the_url_and_never_a_container() {
        let mut f = form("http", "", "", "");
        f.use_container = Some("on".into());
        let parsed = parse_form(f).unwrap();
        assert_eq!(parsed.config, serde_json::json!({ "url": "https://x.example/mcp" }));
        assert!(!parsed.use_container && parsed.container_image.is_none());
    }

    #[test]
    fn default_args_follow_the_schema() {
        let schema = serde_json::json!({ "properties": {
            "q": { "type": "string" }, "n": { "type": "integer", "default": 5 }, "flag": { "type": "boolean" }
        }});
        assert_eq!(default_args(&schema), serde_json::json!({ "q": "", "n": 5, "flag": false }));
    }
}
