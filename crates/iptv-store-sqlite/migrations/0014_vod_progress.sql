-- Source-scoped progress survives catalogue refresh but never source deletion.
CREATE TABLE vod_progress (
    source_id INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK(kind IN ('movie','series','episode')),
    content_id TEXT NOT NULL,
    parent_id TEXT,
    position_ms INTEGER NOT NULL DEFAULT 0 CHECK(position_ms >= 0),
    duration_ms INTEGER CHECK(duration_ms >= 0),
    completed INTEGER NOT NULL DEFAULT 0 CHECK(completed IN (0,1)),
    updated_at INTEGER NOT NULL,
    PRIMARY KEY(source_id,kind,content_id)
) WITHOUT ROWID;
CREATE INDEX vod_progress_recent ON vod_progress(updated_at DESC);

ALTER TABLE vod ADD COLUMN episode_numbers TEXT NOT NULL DEFAULT '[]';
