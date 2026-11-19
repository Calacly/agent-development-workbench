-- LLM provider used for the last message of a conversation; the chat page
-- preselects it (falls back to the default provider if it no longer exists)
ALTER TABLE conversations ADD COLUMN last_provider_id INTEGER;
