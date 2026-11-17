//! Sandboxing configuration for local MCP servers
//!
//! This module provides security controls for local (stdio) MCP servers to prevent
//! unauthorized access and resource exhaustion.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;

/// Sandboxing configuration for MCP servers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    /// Maximum execution time for the server process (in seconds)
    /// If None, no timeout is enforced
    pub timeout_secs: Option<u64>,

    /// Maximum memory usage in MB
    /// If None, no memory limit is enforced (platform-dependent)
    pub max_memory_mb: Option<u64>,

    /// Working directory for the server process
    /// If None, uses current directory
    pub working_directory: Option<PathBuf>,

    /// Environment variables to set/override
    pub environment: HashMap<String, String>,

    /// Whether to restrict network access (platform-dependent)
    /// On Linux, this requires additional setup (network namespaces)
    pub restrict_network: bool,

    /// Maximum number of file descriptors
    /// If None, uses system default
    pub max_file_descriptors: Option<u64>,

    /// Paths that should be readable by the server
    /// Empty means no filesystem restrictions
    pub allowed_read_paths: Vec<PathBuf>,

    /// Paths that should be writable by the server
    /// Empty means no write access
    pub allowed_write_paths: Vec<PathBuf>,

    /// Whether to run in a restricted environment
    /// This enables all security features
    pub enable_restrictions: bool,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        // Create a permissive default config suitable for tools like uvx/uv
        // which need to spawn processes and manage virtual environments
        //
        // IMPORTANT: rlimit restrictions are DISABLED by default because they cause
        // "Resource temporarily unavailable" errors on macOS when uvx tries to:
        // - Fork processes for Python interpreter
        // - Map shared libraries and cache files
        // - Query Python executables in cache directories
        //
        // Users can enable restrictions via SandboxConfig::strict() if needed
        let mut env = HashMap::new();
        // Add UV-specific environment variables to prevent resource issues
        env.insert("UV_NO_SYNC".to_string(), "1".to_string()); // Skip uv self-update checks

        Self {
            // Default 5 minute timeout (not enforced when restrictions disabled)
            timeout_secs: Some(300),
            // Memory limit not enforced when restrictions disabled
            max_memory_mb: Some(2048),
            working_directory: None,
            environment: env,
            restrict_network: false,
            // File descriptors not enforced when restrictions disabled
            max_file_descriptors: Some(1024),
            allowed_read_paths: vec![],
            allowed_write_paths: vec![],
            // Disable rlimit restrictions by default to prevent uvx errors
            enable_restrictions: false,
        }
    }
}

impl SandboxConfig {
    /// Create a permissive sandbox configuration (for trusted servers)
    pub fn permissive() -> Self {
        Self {
            timeout_secs: Some(3600), // 1 hour
            max_memory_mb: None,      // No limit
            working_directory: None,
            environment: HashMap::new(),
            restrict_network: false,
            max_file_descriptors: None,
            allowed_read_paths: vec![],
            allowed_write_paths: vec![],
            enable_restrictions: false,
        }
    }

    /// Create a strict sandbox configuration (for untrusted servers)
    pub fn strict() -> Self {
        Self {
            timeout_secs: Some(60),   // 1 minute
            max_memory_mb: Some(128), // 128MB
            working_directory: None,
            environment: HashMap::new(),
            restrict_network: true,
            max_file_descriptors: Some(64),
            allowed_read_paths: vec![],
            allowed_write_paths: vec![],
            enable_restrictions: true,
        }
    }

    /// Apply sandbox configuration to a Tokio Command
    pub fn configure_command(&self, cmd: &mut Command) {
        // Always set working directory if specified
        if let Some(ref wd) = self.working_directory {
            cmd.current_dir(wd);
        }

        // Always merge environment variables (don't clear existing ones)
        // This allows tools like uvx to access PATH, HOME, and other essential vars
        for (key, value) in &self.environment {
            cmd.env(key, value);
        }

        // Only apply rlimit restrictions if enabled
        if !self.enable_restrictions {
            return;
        }

        // Platform-specific resource limits
        #[cfg(target_family = "unix")]
        {
            // Clone values to avoid capturing self
            let max_memory_mb = self.max_memory_mb;
            let max_file_descriptors = self.max_file_descriptors;
            let timeout_secs = self.timeout_secs;

            // Set resource limits using rlimit
            // SAFETY: pre_exec requires unsafe because it can cause undefined behavior
            // if the closure performs operations that are not async-signal-safe.
            // We only use setrlimit which is documented to be safe for this purpose.
            unsafe {
                cmd.pre_exec(move || {
                    // These are best-effort; some may fail depending on permissions
                    // Limit virtual memory (if configured)
                    if let Some(max_mem) = max_memory_mb {
                        let limit = (max_mem * 1024 * 1024) as libc::rlim_t;
                        let rlim = libc::rlimit {
                            rlim_cur: limit,
                            rlim_max: limit,
                        };
                        libc::setrlimit(libc::RLIMIT_AS, &rlim);
                    }

                    // Limit number of open files
                    if let Some(max_fds) = max_file_descriptors {
                        let rlim = libc::rlimit {
                            rlim_cur: max_fds,
                            rlim_max: max_fds,
                        };
                        libc::setrlimit(libc::RLIMIT_NOFILE, &rlim);
                    }

                    // Limit CPU time (if timeout is set)
                    if let Some(timeout) = timeout_secs {
                        let rlim = libc::rlimit {
                            rlim_cur: timeout,
                            rlim_max: timeout,
                        };
                        libc::setrlimit(libc::RLIMIT_CPU, &rlim);
                    }

                    // Prevent creation of core dumps
                    let rlim_zero = libc::rlimit {
                        rlim_cur: 0,
                        rlim_max: 0,
                    };
                    libc::setrlimit(libc::RLIMIT_CORE, &rlim_zero);

                    // Limit number of processes (increased for tools like uvx)
                    let rlim_nproc = libc::rlimit {
                        rlim_cur: 128,
                        rlim_max: 128,
                    };
                    libc::setrlimit(libc::RLIMIT_NPROC, &rlim_nproc);

                    Ok(())
                });
            }
        }

        #[cfg(target_os = "windows")]
        {
            // Windows-specific job object limits would go here
            // This requires additional Windows API calls
            tracing::warn!("Resource limits not fully supported on Windows");
        }
    }

    /// Get timeout duration
    pub fn timeout_duration(&self) -> Option<Duration> {
        self.timeout_secs.map(Duration::from_secs)
    }

    /// Validate that paths are allowed for reading
    pub fn validate_read_path(&self, path: &PathBuf) -> bool {
        if !self.enable_restrictions || self.allowed_read_paths.is_empty() {
            return true; // No restrictions
        }

        // Check if path is within any allowed read path
        self.allowed_read_paths
            .iter()
            .any(|allowed| path.starts_with(allowed))
    }

    /// Validate that paths are allowed for writing
    pub fn validate_write_path(&self, path: &PathBuf) -> bool {
        if !self.enable_restrictions || self.allowed_write_paths.is_empty() {
            return true; // No restrictions
        }

        // Check if path is within any allowed write path
        self.allowed_write_paths
            .iter()
            .any(|allowed| path.starts_with(allowed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_sandbox() {
        // rlimits are off by default: they break uvx on macOS
        let config = SandboxConfig::default();
        assert!(!config.enable_restrictions);
        assert_eq!(config.timeout_secs, Some(300));
        assert_eq!(config.environment.get("UV_NO_SYNC").map(String::as_str), Some("1"));
    }

    #[test]
    fn test_permissive_sandbox() {
        let config = SandboxConfig::permissive();
        assert!(!config.enable_restrictions);
        assert!(config.max_memory_mb.is_none());
    }

    #[test]
    fn test_strict_sandbox() {
        let config = SandboxConfig::strict();
        assert!(config.enable_restrictions);
        assert_eq!(config.timeout_secs, Some(60));
        assert_eq!(config.max_memory_mb, Some(128));
        assert!(config.restrict_network);
    }

    #[test]
    fn test_path_validation() {
        // Path rules only apply with restrictions enabled
        let mut config = SandboxConfig::strict();
        config.allowed_read_paths = vec![PathBuf::from("/tmp")];
        config.allowed_write_paths = vec![PathBuf::from("/tmp/output")];

        assert!(config.validate_read_path(&PathBuf::from("/tmp/test.txt")));
        assert!(!config.validate_read_path(&PathBuf::from("/etc/passwd")));

        assert!(config.validate_write_path(&PathBuf::from("/tmp/output/file.txt")));
        assert!(!config.validate_write_path(&PathBuf::from("/tmp/file.txt")));
    }

    #[test]
    fn test_no_restrictions() {
        let mut config = SandboxConfig::default();
        config.enable_restrictions = false;

        // All paths should be allowed when restrictions are disabled
        assert!(config.validate_read_path(&PathBuf::from("/etc/passwd")));
        assert!(config.validate_write_path(&PathBuf::from("/etc/passwd")));
    }
}
