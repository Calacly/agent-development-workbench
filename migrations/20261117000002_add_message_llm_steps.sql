-- Trace of the LLM rounds behind an assistant message (JSON array of steps):
-- what was sent, what came back, tokens, duration, and which tool calls it triggered
ALTER TABLE messages ADD COLUMN llm_steps TEXT;
