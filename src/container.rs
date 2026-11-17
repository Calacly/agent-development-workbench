//! Container-based MCP server execution
//!
//! This module provides support for running MCP servers in isolated containers
//! using Podman (or Docker as fallback). This provides significantly better
//! security isolation than process-level sandboxing.

use crate::error::{AppError, AppResult};
use std::process::Stdio;
use tokio::process::Command;

/// Container runtime type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerRuntime {
    Podman,
    Docker,
}

impl ContainerRuntime {
    /// Detect available container runtime
    pub async fn detect() -> Option<Self> {
        // Try Podman first (preferred for rootless operation)
        if Self::is_available("podman").await {
            return Some(ContainerRuntime::Podman);
        }

        // Fallback to Docker
        if Self::is_available("docker").await {
            return Some(ContainerRuntime::Docker);
        }

        None
    }

    /// Check if a command is available
    async fn is_available(cmd: &str) -> bool {
        Command::new(cmd)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .is_ok()
    }

    /// Get the command name
    pub fn command(&self) -> &'static str {
        match self {
            ContainerRuntime::Podman => "podman",
            ContainerRuntime::Docker => "docker",
        }
    }
}

/// Configuration for containerized MCP server
#[derive(Debug, Clone)]
pub struct ContainerConfig {
    /// Container image to use
    pub image: String,
    /// Command to run in container
    pub command: Vec<String>,
    /// Environment variables
    pub env: Vec<(String, String)>,
    /// Memory limit in MB (None = no limit)
    pub memory_mb: Option<u64>,
    /// CPU limit (e.g., 1.0 = 1 CPU core)
    pub cpu_limit: Option<f64>,
    /// Network mode (default: none for isolation)
    pub network: NetworkMode,
    /// Read-only root filesystem
    pub read_only: bool,
    /// Temporary filesystem mounts
    pub tmpfs: Vec<String>,
    /// DNS servers passed as --dns (from CONTAINER_DNS). Empty = runtime default.
    pub dns: Vec<String>,
    /// Max processes and threads in the container (--pids-limit)
    pub pids_limit: u32,
}

/// pids limit for package-manager based servers: uv, npm and the runtimes
/// they start use many threads, which count toward --pids-limit. With 50,
/// uvx aborts with "Resource temporarily unavailable (os error 11)".
const RUNTIME_PIDS_LIMIT: u32 = 256;

/// Parse a comma-separated DNS server list such as "1.1.1.1, 8.8.8.8".
pub fn parse_dns_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// DNS servers for containers from the CONTAINER_DNS environment variable.
/// Needed e.g. with Podman machine on macOS behind a VPN, whose DNS forwarder
/// answers IPv6 (AAAA) lookups with SERVFAIL; musl-based tools (Alpine images,
/// the uv binary) then fail every lookup with EAI_AGAIN.
pub fn dns_from_env() -> Vec<String> {
    std::env::var("CONTAINER_DNS")
        .map(|v| parse_dns_list(&v))
        .unwrap_or_default()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkMode {
    None,
    Bridge,
    Host,
}

impl NetworkMode {
    fn as_str(&self) -> &'static str {
        match self {
            NetworkMode::None => "none",
            NetworkMode::Bridge => "bridge",
            NetworkMode::Host => "host",
        }
    }
}

impl Default for ContainerConfig {
    fn default() -> Self {
        Self {
            image: "ghcr.io/astral-sh/uv:python3.15-trixie-slim".to_string(),
            command: vec![],
            env: vec![],
            memory_mb: Some(512),
            cpu_limit: Some(1.0),
            network: NetworkMode::None,
            read_only: true,
            tmpfs: vec!["/tmp".to_string()],
            dns: dns_from_env(),
            pids_limit: 50,
        }
    }
}

impl ContainerConfig {
    /// Create config for a uvx-based MCP server
    pub fn for_uvx_server(package: &str, args: Vec<String>) -> Self {
        let mut config = Self::default();
        // Use the uv image which already has uv/uvx installed
        config.image = "ghcr.io/astral-sh/uv:python3.15-trixie-slim".to_string();

        // Build command: run uvx directly (uv is already in the image)
        config.command = vec!["uvx".to_string(), package.to_string()];
        config.command.extend(args);

        // uvx needs network access to download packages from PyPI
        config.network = NetworkMode::Bridge;

        // Increase memory for Python/uv operations
        config.memory_mb = Some(1024);

        // uvx needs to write cache and install packages
        config.read_only = false;
        config.pids_limit = RUNTIME_PIDS_LIMIT;

        config
    }

    /// Create config for a npx/npm-based MCP server
    pub fn for_npx_server(package: &str, args: Vec<String>) -> Self {
        let mut config = Self::default();
        config.image = "node:26-slim".to_string();

        // Build command: run npx with the package
        config.command = vec!["npx".to_string(), package.to_string()];
        config.command.extend(args);

        // npm/npx need network access to download packages and connect to services
        config.network = NetworkMode::Bridge;

        // Increase memory for Node.js operations
        config.memory_mb = Some(1024);

        // npm needs to write to cache and node_modules
        config.read_only = false;
        config.pids_limit = RUNTIME_PIDS_LIMIT;

        config
    }

    /// Build container run command
    pub fn build_command(&self, runtime: ContainerRuntime) -> Command {
        let mut cmd = Command::new(runtime.command());

        cmd.arg("run")
            .arg("--rm") // Remove container after exit
            .arg("-i") // Interactive (keep stdin open)
            .arg("--cap-drop=ALL") // Drop all capabilities
            .arg("--security-opt=no-new-privileges"); // Prevent privilege escalation

        // Memory limit
        if let Some(mem) = self.memory_mb {
            cmd.arg(format!("--memory={}m", mem));
        }

        // CPU limit
        if let Some(cpu) = self.cpu_limit {
            cmd.arg(format!("--cpus={}", cpu));
        }

        // Network isolation
        cmd.arg(format!("--network={}", self.network.as_str()));

        // Custom DNS servers (only meaningful with a network)
        if self.network != NetworkMode::None {
            for server in &self.dns {
                cmd.arg(format!("--dns={}", server));
            }
        }

        // Read-only filesystem
        if self.read_only {
            cmd.arg("--read-only");
        }

        // Temporary filesystems
        for tmpfs_path in &self.tmpfs {
            cmd.arg(format!("--tmpfs={}", tmpfs_path));
        }

        // Environment variables
        for (key, value) in &self.env {
            cmd.arg("-e").arg(format!("{}={}", key, value));
        }

        // Process limits
        cmd.arg(format!("--pids-limit={}", self.pids_limit));

        // Image
        cmd.arg(&self.image);

        // Command to run in container
        if !self.command.is_empty() {
            cmd.args(&self.command);
        }

        // Set up stdio for MCP communication
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        cmd
    }

    /// Build container run command with custom parameters
    pub fn build_command_with_params(&self, runtime: ContainerRuntime, custom_params: Vec<String>) -> Command {
        let mut cmd = Command::new(runtime.command());

        cmd.arg("run")
            .arg("--rm") // Remove container after exit
            .arg("-i"); // Interactive (keep stdin open)

        // Add custom parameters first (so they can be overridden by security defaults if needed)
        for param in &custom_params {
            cmd.arg(param);
        }

        // Security defaults
        cmd.arg("--cap-drop=ALL") // Drop all capabilities
            .arg("--security-opt=no-new-privileges"); // Prevent privilege escalation

        // Memory limit
        if let Some(mem) = self.memory_mb {
            cmd.arg(format!("--memory={}m", mem));
        }

        // CPU limit
        if let Some(cpu) = self.cpu_limit {
            cmd.arg(format!("--cpus={}", cpu));
        }

        // Network isolation
        cmd.arg(format!("--network={}", self.network.as_str()));

        // Custom DNS servers (only meaningful with a network)
        if self.network != NetworkMode::None {
            for server in &self.dns {
                cmd.arg(format!("--dns={}", server));
            }
        }

        // Read-only filesystem
        if self.read_only {
            cmd.arg("--read-only");
        }

        // Temporary filesystems
        for tmpfs_path in &self.tmpfs {
            cmd.arg(format!("--tmpfs={}", tmpfs_path));
        }

        // Environment variables
        for (key, value) in &self.env {
            cmd.arg("-e").arg(format!("{}={}", key, value));
        }

        // Process limits
        cmd.arg(format!("--pids-limit={}", self.pids_limit));

        // Image
        cmd.arg(&self.image);

        // Command to run in container
        if !self.command.is_empty() {
            cmd.args(&self.command);
        }

        // Set up stdio for MCP communication
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        cmd
    }
}

/// Check if container runtime is available
pub async fn is_container_available() -> bool {
    ContainerRuntime::detect().await.is_some()
}

/// Get the preferred container runtime
pub async fn get_runtime() -> AppResult<ContainerRuntime> {
    ContainerRuntime::detect()
        .await
        .ok_or_else(|| AppError::BadRequest(
            "No container runtime available. Please install Podman or Docker.".to_string()
        ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_detect_runtime() {
        // This test will pass if either podman or docker is installed
        let runtime = ContainerRuntime::detect().await;
        // Can't assert on specific runtime as it depends on system
        println!("Detected runtime: {:?}", runtime);
    }

    #[test]
    fn test_container_config_default() {
        let config = ContainerConfig::default();
        assert_eq!(config.image, "ghcr.io/astral-sh/uv:python3.15-trixie-slim");
        assert_eq!(config.memory_mb, Some(512));
        assert_eq!(config.network, NetworkMode::None);
        assert!(config.read_only);
    }

    #[test]
    fn test_uvx_server_config() {
        let config = ContainerConfig::for_uvx_server("mcp-server-git", vec![]);
        assert_eq!(config.image, "ghcr.io/astral-sh/uv:python3.15-trixie-slim");
        assert_eq!(config.memory_mb, Some(1024));
        assert!(!config.read_only); // uvx needs write access
        assert_eq!(config.network, NetworkMode::Bridge); // uvx needs network to download packages
    }

    #[test]
    fn test_npx_server_config() {
        let config = ContainerConfig::for_npx_server("mcp-remote", vec!["https://example.com".to_string()]);
        assert_eq!(config.image, "node:26-slim");
        assert_eq!(config.memory_mb, Some(1024));
        assert!(!config.read_only); // npm needs write access
        assert_eq!(config.network, NetworkMode::Bridge); // npm needs network access
    }

    #[test]
    fn test_build_command() {
        let config = ContainerConfig::default();
        let cmd = config.build_command(ContainerRuntime::Podman);

        let cmd_str = format!("{:?}", cmd);
        assert!(cmd_str.contains("podman"));
        assert!(cmd_str.contains("--rm"));
        assert!(cmd_str.contains("--cap-drop=ALL"));
        assert!(cmd_str.contains("--network=none"));
    }

    #[test]
    fn test_parse_dns_list() {
        assert_eq!(parse_dns_list(" 1.1.1.1, 8.8.8.8 ,,"), vec!["1.1.1.1", "8.8.8.8"]);
        assert!(parse_dns_list("").is_empty());
    }

    #[test]
    fn test_dns_flags_only_with_network() {
        let mut config = ContainerConfig::for_npx_server("mcp-remote", vec![]);
        config.dns = vec!["1.1.1.1".to_string()];
        let with_network = format!("{:?}", config.build_command(ContainerRuntime::Podman));
        assert!(with_network.contains("--dns=1.1.1.1"), "{with_network}");

        config.network = NetworkMode::None;
        let without_network = format!("{:?}", config.build_command(ContainerRuntime::Podman));
        assert!(!without_network.contains("--dns"), "{without_network}");
    }

    #[test]
    fn test_pids_limit_per_config() {
        let generic = format!("{:?}", ContainerConfig::default().build_command(ContainerRuntime::Podman));
        assert!(generic.contains("--pids-limit=50"), "{generic}");
        let uvx = format!("{:?}", ContainerConfig::for_uvx_server("mcp-server-time", vec![]).build_command(ContainerRuntime::Podman));
        assert!(uvx.contains("--pids-limit=256"), "{uvx}");
        let npx = format!("{:?}", ContainerConfig::for_npx_server("mcp-remote", vec![]).build_command(ContainerRuntime::Podman));
        assert!(npx.contains("--pids-limit=256"), "{npx}");
    }
}
