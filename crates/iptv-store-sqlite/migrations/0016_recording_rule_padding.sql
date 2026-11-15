-- Guide occurrence identity remains recording_rule_jobs.starts_at; padding
-- affects capture windows only and must survive native host restarts.
ALTER TABLE recording_rules ADD COLUMN lead_seconds INTEGER NOT NULL DEFAULT 0 CHECK(lead_seconds BETWEEN 0 AND 3600);
ALTER TABLE recording_rules ADD COLUMN trail_seconds INTEGER NOT NULL DEFAULT 0 CHECK(trail_seconds BETWEEN 0 AND 3600);
