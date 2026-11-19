-- MCP servers switched off for one conversation. Servers not listed here are
-- on, so servers added later are on in every conversation by default.
CREATE TABLE IF NOT EXISTS conversation_disabled_servers (
    conversation_id INTEGER NOT NULL,
    server_id INTEGER NOT NULL,
    PRIMARY KEY (conversation_id, server_id),
    FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,
    FOREIGN KEY (server_id) REFERENCES mcp_servers(id) ON DELETE CASCADE
);
