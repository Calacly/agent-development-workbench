use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Extension, Json, Router,
};
use serde::Deserialize;

use crate::{
    error::{AppError, AppResult},
    middleware::auth::AuthUser,
    models::mcp_server::{CreateMcpServerRequest, McpServer, McpServerResponse, McpServerStatus, McpServerType, UpdateMcpServerRequest},
    services::mcp_client::McpTransport,
    AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_servers).post(create_server))
        .route("/:id", get(get_server).put(update_server).delete(delete_server))
        .route("/:id/connect", post(connect_server))
        .route("/:id/disconnect", post(disconnect_server))
        .route("/:id/capabilities", get(get_capabilities))
        .route("/:id/tools", get(list_tools))
        .route("/:id/tools/:tool_name", post(call_tool))
        .route("/:id/resources", get(list_resources))
        .route("/:id/resources/read", post(read_resource))
        .route("/:id/prompts", get(list_prompts))
        .route("/:id/prompts/:prompt_name", post(get_prompt))
        .route("/:id/test-tools", post(test_tools_with_llm))
}

// List all MCP servers for the authenticated user
pub(crate) async fn list_servers(
    State(state): State<AppState>,
    Extension(AuthUser { user_id, .. }): Extension<AuthUser>,
) -> AppResult<Json<Vec<McpServerResponse>>> {
    let servers = sqlx::query_as::<_, McpServer>(
        "SELECT * FROM mcp_servers WHERE user_id = ? ORDER BY created_at DESC",
    )
    .bind(user_id)
    .fetch_all(&state.db)
    .await?;

    let responses: Vec<McpServerResponse> = servers
        .into_iter()
        .map(|s| McpServerResponse {
            id: s.id,
            name: s.name,
            server_type: serde_json::from_str(&format!("\"{}\"", s.server_type)).unwrap_or(McpServerType::Stdio),
            config: serde_json::from_str(&s.config).unwrap_or(serde_json::json!({})),
            status: serde_json::from_str(&format!("\"{}\"", s.status)).unwrap_or(McpServerStatus::Stopped),
            use_container: s.use_container,
            container_image: s.container_image,
            container_params: s.container_params,
            created_at: s.created_at,
            updated_at: s.updated_at,
        })
        .collect();

    Ok(Json(responses))
}

// Create a new MCP server
pub(crate) async fn create_server(
    State(state): State<AppState>,
    Extension(AuthUser { user_id, .. }): Extension<AuthUser>,
    Json(req): Json<CreateMcpServerRequest>,
) -> AppResult<(StatusCode, Json<McpServerResponse>)> {
    let server_type = serde_json::to_string(&req.server_type)
        .map_err(|e| AppError::BadRequest(format!("Invalid server type: {e}")))?
        .trim_matches('"')
        .to_string();

    let config = serde_json::to_string(&req.config)
        .map_err(|e| AppError::BadRequest(format!("Invalid config: {e}")))?;

    let server = sqlx::query_as::<_, McpServer>(
        "INSERT INTO mcp_servers (user_id, name, server_type, config, status, use_container, container_image, container_params)
         VALUES (?, ?, ?, ?, 'stopped', ?, ?, ?)
         RETURNING *",
    )
    .bind(user_id)
    .bind(&req.name)
    .bind(&server_type)
    .bind(&config)
    .bind(req.use_container)
    .bind(&req.container_image)
    .bind(&req.container_params)
    .fetch_one(&state.db)
    .await?;

    let response = McpServerResponse {
        id: server.id,
        name: server.name,
        server_type: req.server_type,
        config: req.config,
        status: McpServerStatus::Stopped,
        use_container: server.use_container,
        container_image: server.container_image,
        container_params: server.container_params,
        created_at: server.created_at,
        updated_at: server.updated_at,
    };

    Ok((StatusCode::CREATED, Json(response)))
}

// Get a single MCP server
pub(crate) async fn get_server(
    State(state): State<AppState>,
    Extension(AuthUser { user_id, .. }): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> AppResult<Json<McpServerResponse>> {
    let server = sqlx::query_as::<_, McpServer>(
        "SELECT * FROM mcp_servers WHERE id = ? AND user_id = ?",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("MCP server not found".to_string()))?;

    let response = McpServerResponse {
        id: server.id,
        name: server.name,
        server_type: serde_json::from_str(&format!("\"{}\"", server.server_type))
            .unwrap_or(McpServerType::Stdio),
        config: serde_json::from_str(&server.config).unwrap_or(serde_json::json!({})),
        status: serde_json::from_str(&format!("\"{}\"", server.status))
            .unwrap_or(McpServerStatus::Stopped),
        use_container: server.use_container,
        container_image: server.container_image,
        container_params: server.container_params,
        created_at: server.created_at,
        updated_at: server.updated_at,
    };

    Ok(Json(response))
}

// Update an MCP server
pub(crate) async fn update_server(
    State(state): State<AppState>,
    Extension(AuthUser { user_id, .. }): Extension<AuthUser>,
    Path(id): Path<i64>,
    Json(req): Json<UpdateMcpServerRequest>,
) -> AppResult<Json<McpServerResponse>> {
    // Check if server exists and belongs to user
    let server = sqlx::query_as::<_, McpServer>(
        "SELECT * FROM mcp_servers WHERE id = ? AND user_id = ?",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("MCP server not found".to_string()))?;

    // Update fields
    let name = req.name.unwrap_or(server.name);
    let config = if let Some(cfg) = req.config {
        serde_json::to_string(&cfg)
            .map_err(|e| AppError::BadRequest(format!("Invalid config: {e}")))?
    } else {
        server.config
    };
    let use_container = req.use_container.unwrap_or(server.use_container);
    let container_image = req.container_image.or(server.container_image);
    let container_params = req.container_params.or(server.container_params);

    let updated = sqlx::query_as::<_, McpServer>(
        "UPDATE mcp_servers SET name = ?, config = ?, use_container = ?, container_image = ?, container_params = ?, updated_at = CURRENT_TIMESTAMP
         WHERE id = ? AND user_id = ?
         RETURNING *",
    )
    .bind(&name)
    .bind(&config)
    .bind(use_container)
    .bind(&container_image)
    .bind(&container_params)
    .bind(id)
    .bind(user_id)
    .fetch_one(&state.db)
    .await?;

    let response = McpServerResponse {
        id: updated.id,
        name: updated.name,
        server_type: serde_json::from_str(&format!("\"{}\"", updated.server_type))
            .unwrap_or(McpServerType::Stdio),
        config: serde_json::from_str(&updated.config).unwrap_or(serde_json::json!({})),
        status: serde_json::from_str(&format!("\"{}\"", updated.status))
            .unwrap_or(McpServerStatus::Stopped),
        use_container: updated.use_container,
        container_image: updated.container_image,
        container_params: updated.container_params,
        created_at: updated.created_at,
        updated_at: updated.updated_at,
    };

    Ok(Json(response))
}

// Delete an MCP server
pub(crate) async fn delete_server(
    State(state): State<AppState>,
    Extension(AuthUser { user_id, .. }): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    // Disconnect if connected
    let _ = state.mcp_service.disconnect_server(id).await;

    let result = sqlx::query("DELETE FROM mcp_servers WHERE id = ? AND user_id = ?")
        .bind(id)
        .bind(user_id)
        .execute(&state.db)
        .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("MCP server not found".to_string()));
    }

    Ok(StatusCode::NO_CONTENT)
}

// Connect to an MCP server
pub(crate) async fn connect_server(
    State(state): State<AppState>,
    Extension(AuthUser { user_id, .. }): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> AppResult<Json<serde_json::Value>> {
    let server = sqlx::query_as::<_, McpServer>(
        "SELECT * FROM mcp_servers WHERE id = ? AND user_id = ?",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("MCP server not found".to_string()))?;

    // Parse config
    let config: serde_json::Value = serde_json::from_str(&server.config)
        .map_err(|e| AppError::BadRequest(format!("Invalid server config: {e}")))?;

    // Create transport based on server type
    let transport = match server.server_type.as_str() {
        "stdio" => {
            let command = config["command"]
                .as_str()
                .ok_or_else(|| AppError::BadRequest("Missing 'command' in config".to_string()))?
                .to_string();
            let args = config["args"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            let env = config["env"].as_object().map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            });

            McpTransport::Stdio { command, args, env }
        }
        "http" => {
            let url = config["url"]
                .as_str()
                .ok_or_else(|| AppError::BadRequest("Missing 'url' in config".to_string()))?
                .to_string();
            let bearer_token = config["bearer_token"]
                .as_str()
                .map(|s| s.to_string());
            McpTransport::Http { url, bearer_token }
        }
        _ => {
            return Err(AppError::BadRequest(format!(
                "Unsupported server type: {}",
                server.server_type
            )));
        }
    };

    // Connect using the MCP client service
    // TODO: Allow users to configure sandbox settings per-server
    // For now, use default sandbox configuration for security
    let protocol_version = state
        .mcp_service
        .connect_server(
            id,
            server.name.clone(),
            transport,
            None, // sandbox config
            server.use_container,
            server.container_image,
            server.container_params,
        )
        .await?;

    // Update status in database
    sqlx::query("UPDATE mcp_servers SET status = 'running', updated_at = CURRENT_TIMESTAMP WHERE id = ?")
        .bind(id)
        .execute(&state.db)
        .await?;

    Ok(Json(serde_json::json!({
        "status": "connected",
        "protocol_version": protocol_version,
    })))
}

// Disconnect from an MCP server
pub(crate) async fn disconnect_server(
    State(state): State<AppState>,
    Extension(AuthUser { user_id, .. }): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> AppResult<Json<serde_json::Value>> {
    // Check if server exists and belongs to user
    let _server = sqlx::query_as::<_, McpServer>(
        "SELECT * FROM mcp_servers WHERE id = ? AND user_id = ?",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("MCP server not found".to_string()))?;

    // Disconnect
    state.mcp_service.disconnect_server(id).await?;

    // Update status in database
    sqlx::query("UPDATE mcp_servers SET status = 'stopped', updated_at = CURRENT_TIMESTAMP WHERE id = ?")
        .bind(id)
        .execute(&state.db)
        .await?;

    Ok(Json(serde_json::json!({ "status": "disconnected" })))
}

// Get capabilities of an MCP server (which features it supports)
pub(crate) async fn get_capabilities(
    State(state): State<AppState>,
    Extension(AuthUser { user_id, .. }): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> AppResult<Json<serde_json::Value>> {
    // Verify server belongs to user
    let _server = sqlx::query_as::<_, McpServer>(
        "SELECT * FROM mcp_servers WHERE id = ? AND user_id = ?",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("MCP server not found".to_string()))?;

    // Check each capability by trying to list items
    let has_tools = state.mcp_service.list_tools(id).await.is_ok();
    let has_resources = state.mcp_service.list_resources(id).await.is_ok();
    let has_prompts = state.mcp_service.list_prompts(id).await.is_ok();

    Ok(Json(serde_json::json!({
        "tools": has_tools,
        "resources": has_resources,
        "prompts": has_prompts
    })))
}

// List tools from an MCP server
pub(crate) async fn list_tools(
    State(state): State<AppState>,
    Extension(AuthUser { user_id, .. }): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> AppResult<Json<serde_json::Value>> {
    // Verify server belongs to user
    let _server = sqlx::query_as::<_, McpServer>(
        "SELECT * FROM mcp_servers WHERE id = ? AND user_id = ?",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("MCP server not found".to_string()))?;

    let tools = state.mcp_service.list_tools(id).await?;

    Ok(Json(serde_json::json!({ "tools": tools })))
}

#[derive(Debug, Deserialize)]
pub(crate) struct CallToolRequest {
    pub arguments: serde_json::Value,
}

// Call a tool on an MCP server
pub(crate) async fn call_tool(
    State(state): State<AppState>,
    Extension(AuthUser { user_id, .. }): Extension<AuthUser>,
    Path((id, tool_name)): Path<(i64, String)>,
    Json(req): Json<CallToolRequest>,
) -> AppResult<Json<serde_json::Value>> {
    // Verify server belongs to user
    let _server = sqlx::query_as::<_, McpServer>(
        "SELECT * FROM mcp_servers WHERE id = ? AND user_id = ?",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("MCP server not found".to_string()))?;

    let result = state
        .mcp_service
        .call_tool(id, tool_name, req.arguments)
        .await?;

    Ok(Json(serde_json::to_value(result).unwrap_or(serde_json::json!({}))))
}

// List resources from an MCP server
pub(crate) async fn list_resources(
    State(state): State<AppState>,
    Extension(AuthUser { user_id, .. }): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> AppResult<Json<serde_json::Value>> {
    // Verify server belongs to user
    let _server = sqlx::query_as::<_, McpServer>(
        "SELECT * FROM mcp_servers WHERE id = ? AND user_id = ?",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("MCP server not found".to_string()))?;

    let resources = state.mcp_service.list_resources(id).await?;

    Ok(Json(serde_json::json!({ "resources": resources })))
}

#[derive(Debug, Deserialize)]
pub(crate) struct ReadResourceRequest {
    pub uri: String,
}

// Read a resource from an MCP server
pub(crate) async fn read_resource(
    State(state): State<AppState>,
    Extension(AuthUser { user_id, .. }): Extension<AuthUser>,
    Path(id): Path<i64>,
    Json(req): Json<ReadResourceRequest>,
) -> AppResult<Json<serde_json::Value>> {
    // Verify server belongs to user
    let _server = sqlx::query_as::<_, McpServer>(
        "SELECT * FROM mcp_servers WHERE id = ? AND user_id = ?",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("MCP server not found".to_string()))?;

    let result = state.mcp_service.read_resource(id, req.uri).await?;

    Ok(Json(serde_json::to_value(result).unwrap_or(serde_json::json!({}))))
}

// List prompts from an MCP server
pub(crate) async fn list_prompts(
    State(state): State<AppState>,
    Extension(AuthUser { user_id, .. }): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> AppResult<Json<serde_json::Value>> {
    // Verify server belongs to user
    let _server = sqlx::query_as::<_, McpServer>(
        "SELECT * FROM mcp_servers WHERE id = ? AND user_id = ?",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("MCP server not found".to_string()))?;

    let prompts = state.mcp_service.list_prompts(id).await?;

    Ok(Json(serde_json::json!({ "prompts": prompts })))
}

#[derive(Debug, Deserialize)]
pub(crate) struct GetPromptRequest {
    pub arguments: Option<serde_json::Value>,
}

// Get a prompt from an MCP server
pub(crate) async fn get_prompt(
    State(state): State<AppState>,
    Extension(AuthUser { user_id, .. }): Extension<AuthUser>,
    Path((id, prompt_name)): Path<(i64, String)>,
    Json(req): Json<GetPromptRequest>,
) -> AppResult<Json<serde_json::Value>> {
    // Verify server belongs to user
    let _server = sqlx::query_as::<_, McpServer>(
        "SELECT * FROM mcp_servers WHERE id = ? AND user_id = ?",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("MCP server not found".to_string()))?;

    let result = state
        .mcp_service
        .get_prompt(id, prompt_name, req.arguments)
        .await?;

    Ok(Json(serde_json::to_value(result).unwrap_or(serde_json::json!({}))))
}

#[derive(Debug, Deserialize)]
pub(crate) struct TestToolsRequest {
    pub provider_id: i64,
    pub prompt: String,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct TestToolsResponse {
    pub response: String,
    pub tool_calls: Vec<ToolCallResult>,
    /// LLM failure, e.g. a 429 rate limit (tool results so far are kept)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Every LLM round: what was sent, what came back, tokens, duration
    pub llm_steps: Vec<crate::models::conversation::LlmStep>,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct ToolCallResult {
    pub name: String,
    pub arguments: serde_json::Value,
    pub result: Option<serde_json::Value>,
}

/// Upper bound for LLM -> tool -> LLM rounds in a tool test
const MAX_TOOL_ROUNDS: usize = 5;

// Test tools with an LLM
pub(crate) async fn test_tools_with_llm(
    State(state): State<AppState>,
    Extension(AuthUser { user_id, .. }): Extension<AuthUser>,
    Path(id): Path<i64>,
    Json(req): Json<TestToolsRequest>,
) -> AppResult<Json<TestToolsResponse>> {
    use crate::models::llm_provider::LlmProvider;
    use crate::services::llm_client::LlmClient;

    // Verify server belongs to user
    let _server = sqlx::query_as::<_, McpServer>(
        "SELECT * FROM mcp_servers WHERE id = ? AND user_id = ?",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("MCP server not found".to_string()))?;

    // Get LLM provider
    let provider = sqlx::query_as::<_, LlmProvider>(
        "SELECT * FROM llm_providers WHERE id = ? AND user_id = ?",
    )
    .bind(req.provider_id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("LLM provider not found".to_string()))?;

    // Decrypt API key
    let api_key = provider.api_key(&state.config)?;

    // Get tools from this MCP server
    let mcp_tools = state.mcp_service.list_tools(id).await?;
    let openai_tools = LlmClient::convert_mcp_tools_to_openai(&mcp_tools);

    // Create LLM client
    let tool_cap = provider.tool_result_token_cap();
    let model_name = provider.model.clone().unwrap_or_default();
    let llm_client = LlmClient::new(
        provider.base_url.clone(),
        api_key,
        provider.model.clone().unwrap_or_default(),
    );

    // Same loop as the chat: run the requested tools, send their results back,
    // and repeat until the LLM answers with text. Without the second round the
    // answer to a tool call is empty.
    let mut messages = vec![crate::services::llm_client::Message {
        role: "user".to_string(),
        content: Some(req.prompt),
        tool_calls: None,
        tool_call_id: None,
    }];
    let mut tool_call_results = Vec::new();
    let mut llm_steps: Vec<crate::models::conversation::LlmStep> = Vec::new();
    let mut sent_until = 0;
    let offered_tools = Some(openai_tools.clone());

    for _ in 0..MAX_TOOL_ROUNDS {
        let request = crate::services::llm_client::request_preview(&model_name, &messages, &offered_tools, sent_until);
        sent_until = messages.len();
        let started = std::time::Instant::now();
        let result = llm_client
            .chat_completion(messages.clone(), Some(openai_tools.clone()), None, None)
            .await;
        let mut step = crate::models::conversation::LlmStep {
            round: llm_steps.len() as u32 + 1,
            request,
            response: None,
            usage: None,
            duration_ms: started.elapsed().as_millis() as u64,
            error: None,
            tool_call_indices: Vec::new(),
        };
        let response = match result {
            Ok(response) => {
                let (preview, usage) = crate::services::llm_client::response_preview(&response);
                step.response = Some(preview);
                step.usage = Some(usage);
                llm_steps.push(step);
                response
            }
            // Return the failed round with everything so far, so the page can show it
            Err(e) => {
                let message = match e {
                    AppError::Internal(message) => message,
                    other => other.to_string(),
                };
                step.error = Some(message.clone());
                llm_steps.push(step);
                return Ok(Json(TestToolsResponse {
                    response: String::new(),
                    tool_calls: tool_call_results,
                    error: Some(message),
                    llm_steps,
                }));
            }
        };
        let choice = response
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| AppError::Internal("No response from LLM".to_string()))?;

        let Some(tool_calls) = choice.message.tool_calls.filter(|calls| !calls.is_empty()) else {
            return Ok(Json(TestToolsResponse {
                response: choice.message.content.unwrap_or_default(),
                tool_calls: tool_call_results,
                error: None,
                llm_steps,
            }));
        };
        let tools_before_round = tool_call_results.len();

        messages.push(crate::services::llm_client::Message {
            role: "assistant".to_string(),
            content: choice.message.content.clone(),
            tool_calls: Some(tool_calls.clone()),
            tool_call_id: None,
        });

        for tool_call in tool_calls {
            let tool_name = tool_call.function.name.clone();
            let arguments = tool_call.function.arguments.clone();
            let result = match state.mcp_service.call_tool(id, tool_name.clone(), arguments.clone()).await {
                Ok(r) => serde_json::to_value(r).unwrap_or(serde_json::json!({})),
                Err(e) => serde_json::json!({"error": e.to_string()}),
            };

            messages.push(crate::services::llm_client::Message {
                role: "tool".to_string(),
                content: Some(crate::services::llm_client::cap_tool_result(
                    &crate::services::llm_client::tool_result_text(&result),
                    tool_cap,
                )),
                tool_calls: None,
                tool_call_id: Some(tool_call.id.clone()),
            });
            tool_call_results.push(ToolCallResult {
                name: tool_name,
                arguments,
                result: Some(result),
            });
        }
        if let Some(step) = llm_steps.last_mut() {
            step.tool_call_indices = (tools_before_round..tool_call_results.len()).collect();
        }
    }

    Err(AppError::Internal(format!(
        "LLM still requested tools after {MAX_TOOL_ROUNDS} rounds"
    )))
}
