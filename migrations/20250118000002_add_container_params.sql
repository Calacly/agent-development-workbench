-- Add container_params field to store additional container arguments
-- Examples: "-v /my/data:/data --cap-add SYS_ADMIN"
ALTER TABLE mcp_servers ADD COLUMN container_params TEXT;
