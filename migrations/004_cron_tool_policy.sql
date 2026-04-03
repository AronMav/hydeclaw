-- migrations/004_cron_tool_policy.sql
ALTER TABLE scheduled_jobs ADD COLUMN IF NOT EXISTS tool_policy JSONB DEFAULT NULL;

COMMENT ON COLUMN scheduled_jobs.tool_policy IS
  'Optional tool policy override for this job. Format: {"allow": ["tool1"], "deny": ["tool2"]}. Applied on top of agent tool policy.';
