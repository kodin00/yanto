ALTER TABLE ai_providers DROP CONSTRAINT IF EXISTS ai_providers_protocol_check;
ALTER TABLE ai_providers ADD CONSTRAINT ai_providers_protocol_check
  CHECK (protocol IN ('openai_responses', 'openai_chat', 'anthropic_messages', 'codex_account'));
ALTER TABLE agent_tasks ADD COLUMN IF NOT EXISTS codex_thread_id text;
