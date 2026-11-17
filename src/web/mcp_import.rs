//! Parse MCP client config files (e.g. the mcp.json download from
//! try-mcp.dev) into servers this app can create.
//!
//! Accepted shapes:
//!   { "mcpServers": { "<name>": { "command", "args", "env", "disabled", ... } } }
//!   { "servers":    { "<name>": { ... } } }                       (VS Code style)
//! Entries with "url" (or "serverUrl") become HTTP servers.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportedServer {
    pub name: String,
    pub server_type: String,
    /// {command, args, env} for stdio, {url} for http
    pub config: Value,
    pub disabled: bool,
}

#[derive(Debug, PartialEq)]
pub struct ParseResult {
    pub servers: Vec<ImportedServer>,
    /// Entries that were skipped, with the reason
    pub skipped: Vec<String>,
}

pub fn parse_mcp_config(text: &str) -> Result<ParseResult, String> {
    let data: Value = serde_json::from_str(text).map_err(|e| format!("Not valid JSON: {e}"))?;
    let map: &Map<String, Value> = data
        .get("mcpServers")
        .or_else(|| data.get("servers"))
        .and_then(Value::as_object)
        .ok_or_else(|| "No \"mcpServers\" (or \"servers\") object found in the file.".to_string())?;

    let mut servers = Vec::new();
    let mut skipped = Vec::new();
    for (name, entry) in map {
        let Some(entry) = entry.as_object() else {
            skipped.push(format!("{name}: entry is not an object"));
            continue;
        };
        let disabled = entry.get("disabled").and_then(Value::as_bool).unwrap_or(false);
        let url = entry.get("url").or_else(|| entry.get("serverUrl")).and_then(Value::as_str).unwrap_or("");

        match entry.get("command").and_then(Value::as_str).map(str::trim).filter(|c| !c.is_empty()) {
            Some(command) => {
                let args: Vec<Value> = entry
                    .get("args")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().map(|v| Value::String(v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))).collect())
                    .unwrap_or_default();
                let env: Map<String, Value> = entry
                    .get("env")
                    .and_then(Value::as_object)
                    .map(|e| {
                        e.iter()
                            .map(|(k, v)| (k.clone(), Value::String(v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))))
                            .collect()
                    })
                    .unwrap_or_default();
                servers.push(ImportedServer {
                    name: name.clone(),
                    server_type: "stdio".into(),
                    config: serde_json::json!({ "command": command, "args": args, "env": env }),
                    disabled,
                });
            }
            None if !url.is_empty() => servers.push(ImportedServer {
                name: name.clone(),
                server_type: "http".into(),
                config: serde_json::json!({ "url": url }),
                disabled,
            }),
            None => skipped.push(format!("{name}: neither \"command\" nor \"url\" set")),
        }
    }
    Ok(ParseResult { servers, skipped })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_a_try_mcp_export_as_stdio_servers() {
        let text = json!({ "mcpServers": {
            "try-mcp-emoji-oracle": { "command": "npx", "args": ["mcp-remote", "https://try-mcp.dev/services/emoji-oracle/v1"], "disabled": false, "autoApprove": ["get_emoji_oracle"] },
            "try-mcp-timestamp": { "command": "npx", "args": ["mcp-remote", "https://try-mcp.dev/services/timestamp/v1"], "disabled": false }
        }})
        .to_string();
        let result = parse_mcp_config(&text).unwrap();
        assert!(result.skipped.is_empty());
        assert_eq!(result.servers.len(), 2);
        assert_eq!(result.servers[0].name, "try-mcp-emoji-oracle");
        assert_eq!(result.servers[0].server_type, "stdio");
        assert_eq!(
            result.servers[0].config,
            json!({ "command": "npx", "args": ["mcp-remote", "https://try-mcp.dev/services/emoji-oracle/v1"], "env": {} })
        );
    }

    #[test]
    fn keeps_env_as_strings_and_marks_disabled_entries() {
        let text = json!({ "mcpServers": { "git": { "command": "uvx", "args": ["mcp-server-git"], "env": { "DEBUG": 1 }, "disabled": true } } }).to_string();
        let server = &parse_mcp_config(&text).unwrap().servers[0];
        assert_eq!(server.config, json!({ "command": "uvx", "args": ["mcp-server-git"], "env": { "DEBUG": "1" } }));
        assert!(server.disabled);
    }

    #[test]
    fn turns_url_entries_into_http_servers_and_accepts_the_servers_key() {
        let text = json!({ "servers": { "a": { "url": "https://a.example/mcp" }, "b": { "type": "sse", "serverUrl": "https://b.example/sse" } } }).to_string();
        let servers = parse_mcp_config(&text).unwrap().servers;
        let summary: Vec<(String, String, Value)> =
            servers.into_iter().map(|s| (s.name, s.server_type, s.config)).collect();
        assert_eq!(
            summary,
            vec![
                ("a".into(), "http".into(), json!({ "url": "https://a.example/mcp" })),
                ("b".into(), "http".into(), json!({ "url": "https://b.example/sse" })),
            ]
        );
    }

    #[test]
    fn skips_entries_without_command_or_url() {
        let text = json!({ "mcpServers": { "ok": { "command": "x" }, "bad": { "args": [] }, "worse": 3 } }).to_string();
        let result = parse_mcp_config(&text).unwrap();
        assert_eq!(result.servers.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), vec!["ok"]);
        assert_eq!(result.skipped, vec!["bad: neither \"command\" nor \"url\" set", "worse: entry is not an object"]);
    }

    #[test]
    fn rejects_invalid_json_and_files_without_a_server_map() {
        assert!(parse_mcp_config("{nope").unwrap_err().starts_with("Not valid JSON"));
        assert!(parse_mcp_config("{\"foo\": 1}").unwrap_err().starts_with("No \"mcpServers\""));
    }
}
