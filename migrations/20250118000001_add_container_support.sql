-- Add container support to MCP servers
-- Migration: 20250118000001

-- Add use_container column to enable Podman/Docker containerization
ALTER TABLE mcp_servers ADD COLUMN use_container BOOLEAN NOT NULL DEFAULT 0;

-- Add container_image column for custom container images
ALTER TABLE mcp_servers ADD COLUMN container_image TEXT;
