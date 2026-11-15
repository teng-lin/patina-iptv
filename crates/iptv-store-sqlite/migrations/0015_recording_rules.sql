-- Recurrence is durable intent, independent of any materialized programme.
CREATE TABLE recording_rules (
    id INTEGER PRIMARY KEY,
    source_id INTEGER NOT NULL,
    source_incarnation TEXT NOT NULL,
    channel_id TEXT NOT NULL,
    channel_name TEXT NOT NULL,
    title TEXT NOT NULL,
    starts_at INTEGER NOT NULL,
    ends_at INTEGER NOT NULL,
    local_start INTEGER NOT NULL,
    repeat TEXT NOT NULL CHECK(repeat IN ('daily','weekly','series')),
    enabled INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0,1))
);
CREATE TABLE recording_rule_jobs (
    job_id INTEGER PRIMARY KEY,
    rule_id INTEGER NOT NULL REFERENCES recording_rules(id) ON DELETE CASCADE,
    starts_at INTEGER NOT NULL,
    UNIQUE(rule_id, starts_at)
);
