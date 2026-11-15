-- Jobs survive catalogue replacement and source deletion. They store identities,
-- not resolved URLs, provider credentials, or filesystem paths.
CREATE TABLE recording_jobs (
    id INTEGER PRIMARY KEY,
    source_id INTEGER NOT NULL,
    channel_id TEXT NOT NULL,
    channel_name TEXT NOT NULL,
    title TEXT NOT NULL,
    starts_at INTEGER NOT NULL CHECK(starts_at >= 0),
    ends_at INTEGER NOT NULL CHECK(ends_at > starts_at AND ends_at - starts_at <= 86400),
    status TEXT NOT NULL CHECK(status IN
        ('scheduled','recording','completed','partial','failed','cancelled','missed')),
    bytes INTEGER NOT NULL DEFAULT 0 CHECK(bytes >= 0),
    message TEXT NOT NULL DEFAULT '',
    source_incarnation TEXT NOT NULL
);
CREATE INDEX recording_due ON recording_jobs(status, starts_at);
CREATE UNIQUE INDEX recording_unique_active ON recording_jobs(source_id, channel_id, starts_at, ends_at)
    WHERE status IN ('scheduled','recording');
