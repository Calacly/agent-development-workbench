-- Token limits per LLM provider
-- rate_limit_tokens:      tokens per minute reported by the provider (x-ratelimit-limit-tokens)
-- rate_limit_checked_at:  when that value was last read
-- max_tool_result_tokens: cap for a tool result sent back to the LLM (NULL = automatic)
ALTER TABLE llm_providers ADD COLUMN rate_limit_tokens INTEGER;
ALTER TABLE llm_providers ADD COLUMN rate_limit_checked_at DATETIME;
ALTER TABLE llm_providers ADD COLUMN max_tool_result_tokens INTEGER;
