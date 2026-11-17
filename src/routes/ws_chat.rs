use axum::{
    extract::{
        ws::{Message as WsMessage, WebSocket, WebSocketUpgrade},
        Path, State,
    },
    response::Response,
};
use futures::{sink::SinkExt, stream::StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    error::AppError,
    middleware::auth::AuthUser,
    models::{
        conversation::{Conversation, LlmStep, Message, StoredToolCall},
        llm_provider::LlmProvider,
    },
    services::llm_client::LlmClient,
    AppState,
};

#[derive(Debug, Deserialize)]
struct WsChatRequest {
    content: String,
    provider_id: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum WsChatEvent {
    #[serde(rename = "connected")]
    Connected { message: String },
    #[serde(rename = "user_message")]
    UserMessage { message_id: i64, content: String },
    #[serde(rename = "assistant_chunk")]
    AssistantChunk { content: String },
    #[serde(rename = "assistant_complete")]
    AssistantComplete {
        message_id: i64,
        content: String,
        tool_calls: Vec<StoredToolCall>,
        llm_steps: Vec<LlmStep>,
    },
    /// One LLM round finished (sent before the tools it requested run)
    #[serde(rename = "llm_step")]
    LlmStep { step: LlmStep },
    #[serde(rename = "tool_call")]
    ToolCall {
        tool_name: String,
        arguments: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_name: String,
        result: serde_json::Value,
    },
    #[serde(rename = "error")]
    Error { message: String },
}

pub async fn ws_chat_handler(
    ws: WebSocketUpgrade,
    Path(conversation_id): Path<i64>,
    State(state): State<AppState>,
    auth_user: AuthUser,
) -> Response {
    ws.on_upgrade(move |socket| handle_ws_chat(socket, state, conversation_id, auth_user))
}

async fn handle_ws_chat(
    socket: WebSocket,
    state: AppState,
    conversation_id: i64,
    auth_user: AuthUser,
) {
    let (mut sender, mut receiver) = socket.split();

    // Send connected message
    let _ = sender
        .send(WsMessage::Text(
            serde_json::to_string(&WsChatEvent::Connected {
                message: "Connected to chat".to_string(),
            })
            .unwrap(),
        ))
        .await;

    // Listen for messages from client
    while let Some(msg) = receiver.next().await {
        match msg {
            Ok(WsMessage::Text(text)) => {
                // Parse request
                let req: WsChatRequest = match serde_json::from_str(&text) {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = sender
                            .send(WsMessage::Text(
                                serde_json::to_string(&WsChatEvent::Error {
                                    message: format!("Invalid request: {e}"),
                                })
                                .unwrap(),
                            ))
                            .await;
                        continue;
                    }
                };

                // Process the chat message
                if let Err(e) = process_chat_message(
                    &state,
                    &mut sender,
                    conversation_id,
                    auth_user.user_id,
                    req,
                )
                .await
                {
                    let _ = sender
                        .send(WsMessage::Text(
                            serde_json::to_string(&WsChatEvent::Error {
                                message: e.to_string(),
                            })
                            .unwrap(),
                        ))
                        .await;
                }
            }
            Ok(WsMessage::Close(_)) => break,
            Err(e) => {
                tracing::error!("WebSocket error: {e}");
                break;
            }
            _ => {}
        }
    }
}

async fn process_chat_message(
    state: &AppState,
    sender: &mut futures::stream::SplitSink<WebSocket, WsMessage>,
    conversation_id: i64,
    user_id: i64,
    req: WsChatRequest,
) -> Result<(), AppError> {
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
    let user_message = sqlx::query_as::<_, Message>(
        "INSERT INTO messages (conversation_id, role, content) VALUES (?, 'user', ?) RETURNING *",
    )
    .bind(conversation_id)
    .bind(&req.content)
    .fetch_one(&state.db)
    .await?;

    // Send user message event
    sender
        .send(WsMessage::Text(
            serde_json::to_string(&WsChatEvent::UserMessage {
                message_id: user_message.id,
                content: user_message.content.clone(),
            })
            .unwrap(),
        ))
        .await
        .map_err(|_| AppError::Internal("Failed to send WebSocket message".to_string()))?;

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
        .ok_or_else(|| AppError::BadRequest("No default LLM provider configured".to_string()))?
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
    let mut tool_to_server_map: std::collections::HashMap<String, i64> =
        std::collections::HashMap::new();

    tracing::info!(
        "MCP servers with tools: {} servers",
        mcp_tools_map.len()
    );

    for (server_id, tools) in mcp_tools_map {
        tracing::info!(
            "Server {} has {} tools: {:?}",
            server_id,
            tools.len(),
            tools.iter().map(|t| t.name.to_string()).collect::<Vec<_>>()
        );
        for tool in tools {
            tool_to_server_map.insert(tool.name.to_string(), server_id);
            all_mcp_tools.push(tool);
        }
    }

    tracing::info!("Total MCP tools available: {}", all_mcp_tools.len());

    // Convert MCP tools to OpenAI format
    let openai_tools = if !all_mcp_tools.is_empty() {
        let tools = LlmClient::convert_mcp_tools_to_openai(&all_mcp_tools);
        tracing::info!(
            "Converted {} tools for LLM: {:?}",
            tools.len(),
            tools.iter().map(|t| &t.function.name).collect::<Vec<_>>()
        );
        Some(tools)
    } else {
        tracing::warn!("No MCP tools available - LLM will not have tool calling capability");
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

    // Tool calling loop
    let mut max_iterations = 10;
    let mut full_response = String::new();
    let mut stored_tool_calls: Vec<StoredToolCall> = Vec::new();
    let mut executed_tool_calls: std::collections::HashSet<String> = std::collections::HashSet::new();
    // LLM failure after tools already ran (e.g. a 429 rate limit): keep the tool results
    let mut llm_error: Option<String> = None;
    // Trace of every LLM round for the chat timeline. The first round shows
    // only the new user message as "new"; earlier history is counted, not repeated.
    let mut llm_steps: Vec<LlmStep> = Vec::new();
    let mut sent_until = llm_messages.len().saturating_sub(1);

    loop {
        if max_iterations == 0 {
            return Err(AppError::Internal(
                "Too many tool calls, aborting".to_string(),
            ));
        }
        max_iterations -= 1;

        let request = crate::services::llm_client::request_preview(
            &model_name,
            &llm_messages,
            &openai_tools,
            sent_until,
        );
        sent_until = llm_messages.len();
        let started = std::time::Instant::now();
        let result = llm_client
            .chat_completion(llm_messages.clone(), openai_tools.clone(), None, None)
            .await;
        let mut step = LlmStep {
            round: llm_steps.len() as u32 + 1,
            request,
            response: None,
            usage: None,
            duration_ms: started.elapsed().as_millis() as u64,
            error: None,
            tool_call_indices: Vec::new(),
        };
        match &result {
            Ok(response) => {
                let (preview, usage) = crate::services::llm_client::response_preview(response);
                step.response = Some(preview);
                step.usage = Some(usage);
            }
            Err(e) => step.error = Some(e.to_string()),
        }
        let _ = sender
            .send(WsMessage::Text(
                serde_json::to_string(&WsChatEvent::LlmStep { step: step.clone() }).unwrap(),
            ))
            .await;
        llm_steps.push(step);
        let tools_before_round = stored_tool_calls.len();

        let response = match result {
            Ok(response) => {
                // Keep the provider's token limit up to date for the settings page
                if let Err(e) = crate::models::llm_provider::record_rate_limit(
                    &state.db,
                    &provider,
                    response.rate_limit.limit_tokens,
                )
                .await
                {
                    tracing::warn!("Failed to store rate limit: {}", e);
                }
                response
            }
            Err(e) if !stored_tool_calls.is_empty() => {
                llm_error = Some(match e {
                    AppError::Internal(message) => message,
                    other => other.to_string(),
                });
                break;
            }
            Err(e) => return Err(e),
        };

        let choice = response
            .choices
            .first()
            .ok_or_else(|| AppError::Internal("No response from LLM".to_string()))?;

        // Check if LLM wants to call tools
        if let Some(tool_calls) = &choice.message.tool_calls {
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

                // Create a unique key for this tool call to detect duplicates
                let tool_call_key = format!("{}:{}", tool_name, serde_json::to_string(&arguments).unwrap_or_default());

                // Skip if we've already executed this exact tool call
                if executed_tool_calls.contains(&tool_call_key) {
                    tracing::warn!("Skipping duplicate tool call: {}", tool_name);
                    // Add a fake tool result to prevent LLM from calling it again
                    llm_messages.push(crate::services::llm_client::Message {
                        role: "tool".to_string(),
                        content: Some("Already executed".to_string()),
                        tool_calls: None,
                        tool_call_id: Some(tool_call.id.clone()),
                    });
                    continue;
                }
                executed_tool_calls.insert(tool_call_key);

                // Find which server has this tool
                let server_id = tool_to_server_map
                    .get(tool_name)
                    .ok_or_else(|| AppError::Internal(format!("Tool {tool_name} not found")))?;

                // Send tool call event
                sender
                    .send(WsMessage::Text(
                        serde_json::to_string(&WsChatEvent::ToolCall {
                            tool_name: tool_name.clone(),
                            arguments: arguments.clone(),
                        })
                        .unwrap(),
                    ))
                    .await
                    .map_err(|_| {
                        AppError::Internal("Failed to send WebSocket message".to_string())
                    })?;

                // Call the tool
                let tool_result = state
                    .mcp_service
                    .call_tool(*server_id, tool_name.clone(), arguments.clone())
                    .await?;

                // Store tool call information for database
                stored_tool_calls.push(StoredToolCall {
                    tool_name: tool_name.clone(),
                    arguments: arguments.clone(),
                    result: Some(json!(tool_result)),
                });

                // Send tool result event
                sender
                    .send(WsMessage::Text(
                        serde_json::to_string(&WsChatEvent::ToolResult {
                            tool_name: tool_name.clone(),
                            result: json!(tool_result),
                        })
                        .unwrap(),
                    ))
                    .await
                    .map_err(|_| {
                        AppError::Internal("Failed to send WebSocket message".to_string())
                    })?;

                // Extract text content from the MCP tool result
                let tool_result_content = crate::services::llm_client::cap_tool_result(
                    &crate::services::llm_client::tool_result_text(&json!(tool_result)),
                    tool_cap,
                );

                llm_messages.push(crate::services::llm_client::Message {
                    role: "tool".to_string(),
                    content: Some(tool_result_content.clone()),
                    tool_calls: None,
                    tool_call_id: Some(tool_call.id.clone()),
                });
            }
            if let Some(step) = llm_steps.last_mut() {
                step.tool_call_indices = (tools_before_round..stored_tool_calls.len()).collect();
            }
        } else if let Some(content) = &choice.message.content {
            // No tool calls, we have final response
            full_response.push_str(content);

            // Send the content as a chunk
            sender
                .send(WsMessage::Text(
                    serde_json::to_string(&WsChatEvent::AssistantChunk {
                        content: content.clone(),
                    })
                    .unwrap(),
                ))
                .await
                .map_err(|_| {
                    AppError::Internal("Failed to send WebSocket message".to_string())
                })?;

            break;
        } else if choice.finish_reason.as_deref() == Some("stop") {
            // LLM finished without content
            if let Some(last_tool_msg) = llm_messages.iter().rev().find(|m| m.role == "tool") {
                if let Some(tool_content) = &last_tool_msg.content {
                    full_response = tool_content.clone();
                    break;
                }
            }
            return Err(AppError::Internal(
                "LLM stopped without providing content or tool results".to_string(),
            ));
        } else {
            return Err(AppError::Internal(
                "LLM response has no content or tool calls".to_string(),
            ));
        }
    }

    if let Some(error) = &llm_error {
        full_response = format!("The LLM request failed after the tool calls: {error}");
    }

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
    .bind(&full_response)
    .bind(&tool_calls_json)
    .bind(serde_json::to_string(&llm_steps).ok())
    .fetch_one(&state.db)
    .await?;

    // Send complete event
    sender
        .send(WsMessage::Text(
            serde_json::to_string(&WsChatEvent::AssistantComplete {
                message_id: assistant_message.id,
                content: full_response,
                tool_calls: stored_tool_calls,
                llm_steps,
            })
            .unwrap(),
        ))
        .await
        .map_err(|_| AppError::Internal("Failed to send WebSocket message".to_string()))?;

    // Update conversation timestamp
    sqlx::query("UPDATE conversations SET updated_at = CURRENT_TIMESTAMP WHERE id = ?")
        .bind(conversation_id)
        .execute(&state.db)
        .await?;

    Ok(())
}
