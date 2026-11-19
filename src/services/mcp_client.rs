use crate::container::{ContainerConfig, NetworkMode, get_runtime};
use crate::error::{AppError, AppResult};
use crate::sandbox::SandboxConfig;
use rmcp::model::*;
use rmcp::service::RunningService;
use rmcp::transport::{TokioChildProcess, StreamableHttpClientTransport};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::service::{ClientLifecycleMode, ClientServiceExt};
use rmcp::{ClientHandler, RoleClient};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::RwLock;
use chrono::Local;
use std::fs;
use std::path::PathBuf;

/// Parse container parameters string into individual arguments
/// Examples: "-v /my/data:/data --cap-add SYS_ADMIN" -> ["-v", "/my/data:/data", "--cap-add", "SYS_ADMIN"]
fn parse_container_params(params: &str) -> Vec<String> {
    // Simple shell-like parsing: split on whitespace but respect quotes
    let mut result = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut quote_char = ' ';

    for ch in params.chars() {
        match ch {
            '"' | '\'' if !in_quotes => {
                in_quotes = true;
                quote_char = ch;
            }
            c if c == quote_char && in_quotes => {
                in_quotes = false;
            }
            ' ' | '\t' if !in_quotes => {
                if !current.is_empty() {
                    result.push(current.clone());
                    current.clear();
                }
            }
            c => {
                current.push(c);
            }
        }
    }

    if !current.is_empty() {
        result.push(current);
    }

    result
}

/// MCP server transport type
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpTransport {
    /// Local server via stdio (process spawning)
    Stdio {
        command: String,
        args: Vec<String>,
        env: Option<HashMap<String, String>>,
    },
    /// Remote server via HTTP/SSE
    Http {
        url: String,
        bearer_token: Option<String>,
    },
}

/// Lifecycle used for every connection: probe with `server/discover` for the
/// stateless 2026-07-28 revision ("MCP v2"), fall back to the legacy
/// `initialize` handshake for servers that only speak older revisions.
fn lifecycle_mode() -> ClientLifecycleMode {
    ClientLifecycleMode::Auto {
        preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        legacy_version: Some(ProtocolVersion::LATEST_WITH_INITIALIZE),
    }
}

/// Upper bound for starting a stdio server. The first start can be slow:
/// container image pull plus npx/uvx downloading the server package.
const STDIO_START_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

/// Client handler implementation
#[derive(Clone, Debug, Default)]
struct MyClientHandler {
    /// Protocol version for a legacy `initialize` handshake (None = rmcp default)
    initialize_version: Option<ProtocolVersion>,
}

impl ClientHandler for MyClientHandler {
    fn get_info(&self) -> ClientConfig {
        let config = ClientConfig::new(
            ClientCapabilities::default(),
            Implementation::new("mcp-client", env!("CARGO_PKG_VERSION"))
                .with_title("MCP Client Backend"),
        );
        match &self.initialize_version {
            Some(version) => config.with_protocol_version(version.clone()),
            None => config,
        }
    }
}

/// Client wrapper that holds either stdio or StreamableHTTP-based connection
enum McpClientInner {
    Stdio(RunningService<RoleClient, MyClientHandler>),
    StreamableHttp(RunningService<RoleClient, MyClientHandler>),
}

/// MCP client wrapper for a single server connection
pub struct McpClient {
    /// The underlying rmcp running service
    client: McpClientInner,
    /// Server name for identification
    name: String,
    /// Transport configuration
    transport: McpTransport,
}

impl McpClient {
    /// Create a new MCP client connection with optional sandboxing
    pub async fn connect(
        name: String,
        transport: McpTransport,
        sandbox: Option<SandboxConfig>,
        use_container: bool,
        container_image: Option<String>,
        container_params: Option<String>,
    ) -> AppResult<Self> {
        let client = match &transport {
            McpTransport::Stdio {
                command,
                args,
                env,
            } => {
                tracing::info!("Connecting to MCP server '{}' with command: {} {:?}", name, command, args);

                // Choose between container or process-based execution
                let build_cmd = || {
                    if use_container {
                        tracing::info!("Running MCP server '{}' in container", name);
                        Self::build_container_command(command, args, env, container_image.clone(), container_params.clone())
                    } else {
                        tracing::info!("Running MCP server '{}' as process", name);
                        Self::build_process_command(command, args, env, sandbox.clone())
                    }
                };

                McpClientInner::Stdio(Self::start_stdio(&name, build_cmd).await?)
            }
            McpTransport::Http { url, bearer_token } => {
                // Create StreamableHTTP transport
                let mut config = StreamableHttpClientTransportConfig::with_uri(url.clone());
                if let Some(token) = bearer_token {
                    config = config.auth_header(token.clone());
                }

                let transport = StreamableHttpClientTransport::from_config(config);

                // Create client using the StreamableHTTP transport
                let client = MyClientHandler::default()
                    .serve_with_lifecycle(transport, lifecycle_mode())
                    .await
                    .map_err(|e| AppError::Mcp(format!("Failed to connect to HTTP server: {e}")))?;

                McpClientInner::StreamableHttp(client)
            }
        };

        let this = Self {
            client,
            name,
            transport,
        };
        tracing::info!(
            "Connected to MCP server '{}' using protocol version {}",
            this.name,
            this.protocol_version().as_deref().unwrap_or("unknown")
        );
        Ok(this)
    }

    /// Start a stdio server. rmcp's Auto lifecycle gives `server/discover`
    /// only 10 s and then falls back to `initialize` while a slow server is
    /// still starting; its late discover answer then collides with the
    /// initialize request ("conflict initialized response id"). So probe with
    /// discover bounded only by STDIO_START_TIMEOUT, and if the server turns
    /// out to be legacy, start it again with the `initialize` handshake.
    async fn start_stdio(
        name: &str,
        build_cmd: impl Fn() -> AppResult<Command>,
    ) -> AppResult<RunningService<RoleClient, MyClientHandler>> {
        let timeout_error = || {
            AppError::Mcp(format!(
                "Failed to connect to MCP server: no answer within {} s",
                STDIO_START_TIMEOUT.as_secs()
            ))
        };

        let discover = MyClientHandler::default().serve_with_lifecycle(
            TokioChildProcess::new(build_cmd()?)?,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        );
        match tokio::time::timeout(STDIO_START_TIMEOUT, discover).await {
            Ok(Ok(client)) => Ok(client),
            // A correlated JSON-RPC error to server/discover means a legacy server
            Ok(Err(rmcp::service::ClientInitializeError::JsonRpcError(error))) => {
                tracing::info!(
                    "MCP server '{}' does not support server/discover ({}), restarting with initialize",
                    name,
                    error.message
                );
                let legacy = MyClientHandler {
                    initialize_version: Some(ProtocolVersion::LATEST_WITH_INITIALIZE),
                }
                .serve_with_lifecycle(TokioChildProcess::new(build_cmd()?)?, ClientLifecycleMode::Initialize);
                tokio::time::timeout(STDIO_START_TIMEOUT, legacy)
                    .await
                    .map_err(|_| timeout_error())?
                    .map_err(|e| AppError::Mcp(format!("Failed to connect to MCP server: {e}")))
            }
            Ok(Err(e)) => Err(AppError::Mcp(format!("Failed to connect to MCP server: {e}"))),
            Err(_) => Err(timeout_error()),
        }
    }

    /// Protocol version negotiated with the server: `2026-07-28` (MCP v2,
    /// via `server/discover`) or an older revision via `initialize`.
    pub fn protocol_version(&self) -> Option<String> {
        self.get_client()
            .peer_info()
            .map(|info| info.protocol_version.to_string())
    }

    /// Build a containerized command for running MCP server
    fn build_container_command(
        command: &str,
        args: &[String],
        env: &Option<HashMap<String, String>>,
        container_image: Option<String>,
        container_params: Option<String>,
    ) -> AppResult<Command> {
        // Determine command type for appropriate container configuration
        let is_uvx = command == "uvx" || command.ends_with("/uvx");
        let is_npx = command == "npx" || command.ends_with("/npx");
        let is_npm = command == "npm" || command.ends_with("/npm");
        let is_node = command == "node" || command.ends_with("/node");

        let mut config = if is_uvx && !args.is_empty() {
            // Build container config for uvx server (Python)
            ContainerConfig::for_uvx_server(&args[0], args[1..].to_vec())
        } else if is_npx && !args.is_empty() {
            // Build container config for npx server (Node.js)
            ContainerConfig::for_npx_server(&args[0], args[1..].to_vec())
        } else if is_npm || is_node {
            // Generic Node.js container with network access
            let mut config = ContainerConfig::default();
            config.image = "node:26-slim".to_string();
            config.command = vec![command.to_string()];
            config.command.extend(args.iter().cloned());
            config.network = NetworkMode::Bridge; // npm/node need network
            config.read_only = false; // npm needs cache writes
            config.memory_mb = Some(1024);
            config.pids_limit = 256; // node starts many threads
            config
        } else {
            // Generic container config
            let mut config = ContainerConfig::default();
            config.command = vec![command.to_string()];
            config.command.extend(args.iter().cloned());
            config
        };

        // Use custom image if provided
        if let Some(image) = container_image {
            config.image = image;
        }

        // Add environment variables
        if let Some(env_vars) = env {
            for (k, v) in env_vars {
                config.env.push((k.clone(), v.clone()));
            }
        }

        // Parse and add custom container parameters
        let custom_params = if let Some(params_str) = container_params {
            parse_container_params(&params_str)
        } else {
            vec![]
        };

        // Get container runtime
        let runtime = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(get_runtime())
        })?;

        tracing::debug!(
            "Using container runtime: {:?}, image: {}, memory: {:?}MB, network: {:?}, custom_params: {:?}",
            runtime,
            config.image,
            config.memory_mb,
            config.network,
            custom_params
        );

        Ok(config.build_command_with_params(runtime, custom_params))
    }

    /// Build a process-based command with sandbox
    fn build_process_command(
        command: &str,
        args: &[String],
        env: &Option<HashMap<String, String>>,
        sandbox: Option<SandboxConfig>,
    ) -> AppResult<Command> {
        let mut sandbox_config = sandbox.unwrap_or_default();

        // Merge transport environment variables with sandbox config
        if let Some(transport_env) = env {
            for (key, value) in transport_env {
                sandbox_config.environment.insert(key.clone(), value.clone());
            }
        }

        let mut cmd = Command::new(command);
        cmd.args(args);

        // Apply sandbox restrictions
        sandbox_config.configure_command(&mut cmd);

        tracing::debug!(
            "Applied sandbox config: timeout={:?}, memory={:?}MB, file_descriptors={:?}, processes=128, restrictions={}",
            sandbox_config.timeout_secs,
            sandbox_config.max_memory_mb,
            sandbox_config.max_file_descriptors,
            sandbox_config.enable_restrictions
        );

        Ok(cmd)
    }

    /// Get the underlying client reference
    fn get_client(&self) -> &RunningService<RoleClient, MyClientHandler> {
        match &self.client {
            McpClientInner::Stdio(client) => client,
            McpClientInner::StreamableHttp(client) => client,
        }
    }

    /// List all available tools from this server
    pub async fn list_tools(&self) -> AppResult<Vec<Tool>> {
        let tools = self
            .get_client()
            .list_all_tools()
            .await
            .map_err(|e| AppError::Mcp(format!("Failed to list tools: {e}")))?;

        Ok(tools)
    }

    /// Call a tool on this server
    pub async fn call_tool(&self, name: String, arguments: serde_json::Value) -> AppResult<CallToolResult> {
        let response = self
            .get_client()
            .call_tool(match arguments.as_object() {
                Some(args) => CallToolRequestParams::new(name).with_arguments(args.clone()),
                None => CallToolRequestParams::new(name),
            })
            .await
            .map_err(|e| AppError::Mcp(format!("Failed to call tool: {e}")))?;

        Ok(response)
    }

    /// List all resources from this server
    pub async fn list_resources(&self) -> AppResult<Vec<Resource>> {
        let resources = self
            .get_client()
            .list_all_resources()
            .await
            .map_err(|e| AppError::Mcp(format!("Failed to list resources: {e}")))?;

        Ok(resources)
    }

    /// Read a resource from this server
    pub async fn read_resource(&self, uri: String) -> AppResult<ReadResourceResult> {
        let response = self
            .get_client()
            .read_resource(ReadResourceRequestParams::new(uri))
            .await
            .map_err(|e| AppError::Mcp(format!("Failed to read resource: {e}")))?;

        Ok(response)
    }

    /// List all prompts from this server
    pub async fn list_prompts(&self) -> AppResult<Vec<Prompt>> {
        let prompts = self
            .get_client()
            .list_all_prompts()
            .await
            .map_err(|e| AppError::Mcp(format!("Failed to list prompts: {e}")))?;

        Ok(prompts)
    }

    /// Get a prompt from this server
    pub async fn get_prompt(
        &self,
        name: String,
        arguments: Option<serde_json::Value>,
    ) -> AppResult<GetPromptResult> {
        let response = self
            .get_client()
            .get_prompt(match arguments.and_then(|v| v.as_object().cloned()) {
                Some(args) => GetPromptRequestParams::new(name).with_arguments(args),
                None => GetPromptRequestParams::new(name),
            })
            .await
            .map_err(|e| AppError::Mcp(format!("Failed to get prompt: {e}")))?;

        Ok(response)
    }

    /// Get server name
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get transport configuration
    pub fn transport(&self) -> &McpTransport {
        &self.transport
    }
}

/// Service for managing multiple MCP server connections
pub struct McpClientService {
    /// Active client connections indexed by server ID
    clients: Arc<RwLock<HashMap<i64, McpClient>>>,
}

impl McpClientService {
    /// Create a new MCP client service
    pub fn new() -> Self {
        Self {
            clients: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Connect to a new MCP server with optional sandboxing and container support
    pub async fn connect_server(
        &self,
        server_id: i64,
        name: String,
        transport: McpTransport,
        sandbox: Option<SandboxConfig>,
        use_container: bool,
        container_image: Option<String>,
        container_params: Option<String>,
    ) -> AppResult<Option<String>> {
        let client = McpClient::connect(name, transport, sandbox, use_container, container_image, container_params).await?;
        let protocol_version = client.protocol_version();

        let mut clients = self.clients.write().await;
        clients.insert(server_id, client);

        Ok(protocol_version)
    }

    /// Disconnect from an MCP server
    pub async fn disconnect_server(&self, server_id: i64) -> AppResult<()> {
        let mut clients = self.clients.write().await;
        clients
            .remove(&server_id)
            .ok_or_else(|| AppError::NotFound(format!("MCP server {server_id} not found")))?;

        Ok(())
    }

    /// Get a client for a specific server
    async fn get_client(&self, server_id: i64) -> AppResult<Arc<RwLock<HashMap<i64, McpClient>>>> {
        let clients = self.clients.read().await;
        if !clients.contains_key(&server_id) {
            return Err(AppError::NotFound(format!(
                "MCP server {server_id} not connected"
            )));
        }
        drop(clients);
        Ok(Arc::clone(&self.clients))
    }

    /// List tools from a specific server
    pub async fn list_tools(&self, server_id: i64) -> AppResult<Vec<Tool>> {
        let clients_arc = self.get_client(server_id).await?;
        let clients = clients_arc.read().await;
        let client = clients
            .get(&server_id)
            .ok_or_else(|| AppError::NotFound(format!("MCP server {server_id} not found")))?;

        client.list_tools().await
    }

    /// List tools from all connected servers
    /// Tools of the given connected servers. Callers pass the requesting
    /// user's server ids (see `enabled_server_ids` in routes::conversations), so one user's chat never
    /// sees or calls another user's servers.
    pub async fn list_tools_for_servers(&self, server_ids: &[i64]) -> AppResult<HashMap<i64, Vec<Tool>>> {
        let clients = self.clients.read().await;
        let mut all_tools = HashMap::new();

        for (server_id, client) in clients.iter().filter(|(id, _)| server_ids.contains(id)) {
            match client.list_tools().await {
                Ok(tools) => {
                    all_tools.insert(*server_id, tools);
                }
                Err(e) => {
                    tracing::warn!("Failed to list tools from server {}: {}", server_id, e);
                }
            }
        }

        Ok(all_tools)
    }

    /// Call a tool on a specific server
    pub async fn call_tool(
        &self,
        server_id: i64,
        tool_name: String,
        arguments: serde_json::Value,
    ) -> AppResult<CallToolResult> {
        // Create log folder with timestamp and tool name
        let timestamp = Local::now().format("%Y%m%d_%H%M%S_%3f").to_string();
        let log_dir = PathBuf::from("logs").join(format!("{}_mcp-{}", timestamp, tool_name));

        if let Err(e) = fs::create_dir_all(&log_dir) {
            tracing::warn!("Failed to create log directory: {}", e);
        } else {
            // Log request
            let request_log = format!(
                "Server ID: {}\nTool Name: {}\n\nArguments:\n{}\n",
                server_id,
                tool_name,
                serde_json::to_string_pretty(&arguments).unwrap_or_default()
            );

            if let Err(e) = fs::write(log_dir.join("request.txt"), &request_log) {
                tracing::warn!("Failed to write request log: {}", e);
            }
        }

        let clients_arc = self.get_client(server_id).await?;
        let clients = clients_arc.read().await;
        let client = clients
            .get(&server_id)
            .ok_or_else(|| AppError::NotFound(format!("MCP server {server_id} not found")))?;

        let result = client.call_tool(tool_name.clone(), arguments).await;

        // Log response
        match &result {
            Ok(tool_result) => {
                let response_log = format!(
                    "Status: Success\n\nResult:\n{}\n",
                    serde_json::to_string_pretty(&tool_result).unwrap_or_default()
                );
                let _ = fs::write(log_dir.join("response.txt"), &response_log);
            }
            Err(e) => {
                let error_log = format!("Status: Error\n\nError:\n{}\n", e);
                let _ = fs::write(log_dir.join("response.txt"), &error_log);
            }
        }

        result
    }

    /// List resources from a specific server
    pub async fn list_resources(&self, server_id: i64) -> AppResult<Vec<Resource>> {
        let clients_arc = self.get_client(server_id).await?;
        let clients = clients_arc.read().await;
        let client = clients
            .get(&server_id)
            .ok_or_else(|| AppError::NotFound(format!("MCP server {server_id} not found")))?;

        client.list_resources().await
    }

    /// Read a resource from a specific server
    pub async fn read_resource(&self, server_id: i64, uri: String) -> AppResult<ReadResourceResult> {
        let clients_arc = self.get_client(server_id).await?;
        let clients = clients_arc.read().await;
        let client = clients
            .get(&server_id)
            .ok_or_else(|| AppError::NotFound(format!("MCP server {server_id} not found")))?;

        client.read_resource(uri).await
    }

    /// List prompts from a specific server
    pub async fn list_prompts(&self, server_id: i64) -> AppResult<Vec<Prompt>> {
        let clients_arc = self.get_client(server_id).await?;
        let clients = clients_arc.read().await;
        let client = clients
            .get(&server_id)
            .ok_or_else(|| AppError::NotFound(format!("MCP server {server_id} not found")))?;

        client.list_prompts().await
    }

    /// Get a prompt from a specific server
    pub async fn get_prompt(
        &self,
        server_id: i64,
        prompt_name: String,
        arguments: Option<serde_json::Value>,
    ) -> AppResult<GetPromptResult> {
        let clients_arc = self.get_client(server_id).await?;
        let clients = clients_arc.read().await;
        let client = clients
            .get(&server_id)
            .ok_or_else(|| AppError::NotFound(format!("MCP server {server_id} not found")))?;

        client.get_prompt(prompt_name, arguments).await
    }

    /// Check if a server is connected
    pub async fn is_connected(&self, server_id: i64) -> bool {
        let clients = self.clients.read().await;
        clients.contains_key(&server_id)
    }

    /// Get list of all connected server IDs
    pub async fn list_connected_servers(&self) -> Vec<i64> {
        let clients = self.clients.read().await;
        clients.keys().copied().collect()
    }
}

impl Default for McpClientService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_service_creation() {
        let service = McpClientService::new();
        assert_eq!(service.list_connected_servers().await.len(), 0);
    }

    #[tokio::test]
    async fn test_server_connection_state() {
        let service = McpClientService::new();
        assert!(!service.is_connected(1).await);
    }
}
