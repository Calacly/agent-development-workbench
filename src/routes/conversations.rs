use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::get,
    Extension, Json, Router,
};

use crate::{
    error::{AppError, AppResult},
    middleware::auth::AuthUser,
    models::{
        conversation::{Conversation, CreateConversationRequest, LlmStep, Message, SendMessageRequest, StoredToolCall},
        llm_provider::LlmProvider,
    },
    services::llm_client::LlmClient,
    AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_conversations).post(create_conversation))
        .route("/:id", get(get_conversation).delete(delete_conversation))
        .route("/:id/messages", get(list_messages).post(send_message))
}

// List all conversations for the authenticated user
pub(crate) async fn list_conversations(
    State(state): State<AppState>,
    Extension(AuthUser { user_id, .. }): Extension<AuthUser>,
) -> AppResult<Json<Vec<Conversation>>> {
    let conversations = sqlx::query_as::<_, Conversation>(
        "SELECT * FROM conversations WHERE user_id = ? ORDER BY updated_at DESC",
    )
    .bind(user_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(conversations))
}

// Create a new conversation
pub(crate) async fn create_conversation(
    State(state): State<AppState>,
    Extension(AuthUser { user_id, .. }): Extension<AuthUser>,
    Json(req): Json<CreateConversationRequest>,
) -> AppResult<(StatusCode, Json<Conversation>)> {
    let conversation = sqlx::query_as::<_, Conversation>(
        "INSERT INTO conversations (user_id, title) VALUES (?, ?) RETURNING *",
    )
    .bind(user_id)
    .bind(&req.title)
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(conversation)))
}

// Get a single conversation
pub(crate) async fn get_conversation(
    State(state): State<AppState>,
    Extension(AuthUser { user_id, .. }): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> AppResult<Json<Conversation>> {
    let conversation = sqlx::query_as::<_, Conversation>(
        "SELECT * FROM conversations WHERE id = ? AND user_id = ?",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("Conversation not found".to_string()))?;

    Ok(Json(conversation))
}

// Delete a conversation
pub(crate) async fn delete_conversation(
    State(state): State<AppState>,
    Extension(AuthUser { user_id, .. }): Extension<AuthUser>,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    // Delete messages first (foreign key constraint)
    sqlx::query("DELETE FROM messages WHERE conversation_id = ?")
        .bind(id)
        .execute(&state.db)
        .await?;

    let result = sqlx::query("DELETE FROM conversations WHERE id = ? AND user_id = ?")
        .bind(id)
        .bind(user_id)
        .execute(&state.db)
        .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("Conversation not found".to_string()));
    }

    Ok(StatusCode::NO_CONTENT)
}

// List all messages in a conversation
pub(crate) async fn list_messages(
    State(state): State<AppState>,
    Extension(AuthUser { user_id, .. }): Extension<AuthUser>,
    Path(conversation_id): Path<i64>,
) -> AppResult<Json<Vec<Message>>> {
    // Verify conversation belongs to user
    let _conversation = sqlx::query_as::<_, Conversation>(
        "SELECT * FROM conversations WHERE id = ? AND user_id = ?",
    )
    .bind(conversation_id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("Conversation not found".to_string()))?;

    let messages = sqlx::query_as::<_, Message>(
        "SELECT * FROM messages WHERE conversation_id = ? ORDER BY created_at ASC",
    )
    .bind(conversation_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(messages))
}

// Send a message and get LLM response
async fn send_message(
    State(state): State<AppState>,
    Extension(AuthUser { user_id, .. }): Extension<AuthUser>,
    Path(conversation_id): Path<i64>,
    Json(req): Json<SendMessageRequest>,
) -> AppResult<Json<Message>> {
    // Verify conversation belongs to user
    let _conversation = sqlx::query_as::<_, Conversation>(
        "SELECT * FROM conversations WHERE id = ? AND user_id = ?",
    )
    .bind(conversation_id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("Conversation not found".to_string()))?;

    // Save user message
    let _user_message = sqlx::query_as::<_, Message>(
        "INSERT INTO messages (conversation_id, role, content) VALUES (?, 'user', ?) RETURNING *",
    )
    .bind(conversation_id)
    .bind(&req.content)
    .fetch_one(&state.db)
    .await?;

    // Get LLM provider
    let provider = if let Some(provider_id) = req.provider_id {
        sqlx::query_as::<_, LlmProvider>(
            "SELECT * FROM llm_providers WHERE id = ? AND user_id = ?",
        )
        .bind(provider_id)
        .bind(user_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound("LLM provider not found".to_string()))?
    } else {
        // Get default provider
        sqlx::query_as::<_, LlmProvider>(
            "SELECT * FROM llm_providers WHERE user_id = ? AND is_default = 1 LIMIT 1",
        )
        .bind(user_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| {
            AppError::BadRequest("No default LLM provider configured".to_string())
        })?
    };

    // Decrypt API key
    let api_key = provider.api_key(&state.config)?;

    // Get conversation history
    let history = sqlx::query_as::<_, Message>(
        "SELECT * FROM messages WHERE conversation_id = ? ORDER BY created_at ASC",
    )
    .bind(conversation_id)
    .fetch_all(&state.db)
    .await?;

    // Get all tools from connected MCP servers
    // Only this user's servers: tools of other users' servers must not be offered
    let own_servers = crate::services::mcp_client::user_server_ids(&state.db, user_id).await?;
    let mcp_tools_map = state.mcp_service.list_tools_for_servers(&own_servers).await?;
    let mut all_mcp_tools = Vec::new();
    let mut tool_to_server_map: std::collections::HashMap<String, i64> = std::collections::HashMap::new();

    for (server_id, tools) in mcp_tools_map {
        for tool in tools {
            tool_to_server_map.insert(tool.name.to_string(), server_id);
            all_mcp_tools.push(tool);
        }
    }

    // Log available tools
    tracing::info!("Discovered {} MCP tools from {} servers", all_mcp_tools.len(), tool_to_server_map.len());
    for (tool_name, server_id) in &tool_to_server_map {
        tracing::debug!("  Tool '{}' from server {}", tool_name, server_id);
    }

    // Convert MCP tools to OpenAI format
    let openai_tools = if !all_mcp_tools.is_empty() {
        Some(LlmClient::convert_mcp_tools_to_openai(&all_mcp_tools))
    } else {
        None
    };

    // Convert history to LLM messages
    let mut llm_messages: Vec<crate::services::llm_client::Message> = history
        .iter()
        .map(|m| crate::services::llm_client::Message {
            role: m.role.clone(),
            content: Some(m.content.clone()),
            tool_calls: None,
            tool_call_id: None,
        })
        .collect();

    // Call LLM with tool support
    let tool_cap = provider.tool_result_token_cap();
    let model_name = provider.model.clone().unwrap_or_default();
    let llm_client = LlmClient::new(
        provider.base_url.clone(),
        api_key,
        provider.model.clone().unwrap_or_default(),
    );

    // Tool calling loop - keep calling LLM until we get a final response
    let mut max_iterations = 10;
    let mut stored_tool_calls: Vec<StoredToolCall> = Vec::new();
    // Trace of every LLM round for the chat timeline (same as the WebSocket chat)
    let mut llm_steps: Vec<LlmStep> = Vec::new();
    let mut sent_until = llm_messages.len().saturating_sub(1);
    let assistant_content = loop {
        if max_iterations == 0 {
            return Err(AppError::Internal("Too many tool calls, aborting".to_string()));
        }
        max_iterations -= 1;

        let request = crate::services::llm_client::request_preview(&model_name, &llm_messages, &openai_tools, sent_until);
        sent_until = llm_messages.len();
        let started = std::time::Instant::now();
        let response = llm_client
            .chat_completion(llm_messages.clone(), openai_tools.clone(), None, None)
            .await?;
        let (preview, usage) = crate::services::llm_client::response_preview(&response);
        llm_steps.push(LlmStep {
            round: llm_steps.len() as u32 + 1,
            request,
            response: Some(preview),
            usage: Some(usage),
            duration_ms: started.elapsed().as_millis() as u64,
            error: None,
            tool_call_indices: Vec::new(),
        });
        let tools_before_round = stored_tool_calls.len();
        if let Err(e) = crate::models::llm_provider::record_rate_limit(
            &state.db,
            &provider,
            response.rate_limit.limit_tokens,
        )
        .await
        {
            tracing::warn!("Failed to store rate limit: {}", e);
        }

        let choice = response.choices.first()
            .ok_or_else(|| AppError::Internal("No response from LLM".to_string()))?;

        // Check if LLM wants to call tools
        if let Some(tool_calls) = &choice.message.tool_calls {
            tracing::info!("LLM requested {} tool calls", tool_calls.len());

            // Add assistant message with tool calls to history
            llm_messages.push(crate::services::llm_client::Message {
                role: "assistant".to_string(),
                content: choice.message.content.clone(),
                tool_calls: Some(tool_calls.clone()),
                tool_call_id: None,
            });

            // Execute each tool call
            for tool_call in tool_calls {
                let tool_name = &tool_call.function.name;
                let arguments = tool_call.function.arguments.clone();

                // Find which server has this tool
                let server_id = tool_to_server_map.get(tool_name)
                    .ok_or_else(|| AppError::Internal(format!("Tool {tool_name} not found")))?;

                tracing::info!("Calling MCP tool '{}' on server {} with call_id '{}'", tool_name, server_id, tool_call.id);
                tracing::debug!("  Tool arguments: {}", serde_json::to_string_pretty(&arguments).unwrap_or_default());

                // Call the tool
                let tool_result = state.mcp_service
                    .call_tool(*server_id, tool_name.clone(), arguments.clone())
                    .await?;

                // Store tool call information for database
                stored_tool_calls.push(StoredToolCall {
                    tool_name: tool_name.clone(),
                    arguments: arguments.clone(),
                    result: Some(serde_json::json!(tool_result)),
                });

                // Log the result
                tracing::info!("Tool '{}' execution completed for call_id '{}'", tool_name, tool_call.id);
                tracing::debug!("  Tool result: {}", serde_json::to_string_pretty(&tool_result).unwrap_or_default());

                // Text of the MCP tool result, capped for the provider's token limit
                let tool_result_content = crate::services::llm_client::cap_tool_result(
                    &crate::services::llm_client::tool_result_text(&serde_json::json!(tool_result)),
                    tool_cap,
                );

                llm_messages.push(crate::services::llm_client::Message {
                    role: "tool".to_string(),
                    content: Some(tool_result_content.clone()),
                    tool_calls: None,
                    tool_call_id: Some(tool_call.id.clone()),
                });
                tracing::debug!("  Added tool result to messages with content: {}", tool_result_content);
            }
            if let Some(step) = llm_steps.last_mut() {
                step.tool_call_indices = (tools_before_round..stored_tool_calls.len()).collect();
            }
        } else if let Some(content) = &choice.message.content {
            // No tool calls, we have final response
            tracing::info!("LLM provided final response (no more tool calls)");
            break content.clone();
        } else if choice.finish_reason.as_deref() == Some("stop") {
            // LLM finished without content - this can happen when the LLM believes
            // the tool result is sufficient and doesn't need to add anything
            tracing::info!("LLM finished with stop reason but no content after tool call");

            // Find the last tool result and use it as the response
            if let Some(last_tool_msg) = llm_messages.iter().rev().find(|m| m.role == "tool") {
                if let Some(tool_content) = &last_tool_msg.content {
                    tracing::info!("Using last tool result as final response");
                    break tool_content.clone();
                }
            }

            tracing::error!("LLM stopped but no tool result found to use as response");
            return Err(AppError::Internal("LLM stopped without providing content or tool results".to_string()));
        } else {
            tracing::error!("LLM response has no content or tool calls. Full choice: {:?}", choice);
            tracing::error!("Current message history: {}", serde_json::to_string_pretty(&llm_messages).unwrap_or_default());
            return Err(AppError::Internal("LLM response has no content or tool calls".to_string()));
        }
    };

    // Save assistant message with tool_calls
    let tool_calls_json = if !stored_tool_calls.is_empty() {
        Some(serde_json::to_string(&stored_tool_calls).unwrap_or_default())
    } else {
        None
    };

    let assistant_message = sqlx::query_as::<_, Message>(
        "INSERT INTO messages (conversation_id, role, content, tool_calls, llm_steps) VALUES (?, 'assistant', ?, ?, ?) RETURNING *",
    )
    .bind(conversation_id)
    .bind(assistant_content)
    .bind(&tool_calls_json)
    .bind(serde_json::to_string(&llm_steps).ok())
    .fetch_one(&state.db)
    .await?;

    // Update conversation timestamp
    sqlx::query("UPDATE conversations SET updated_at = CURRENT_TIMESTAMP WHERE id = ?")
        .bind(conversation_id)
        .execute(&state.db)
        .await?;

    Ok(Json(assistant_message))
}
