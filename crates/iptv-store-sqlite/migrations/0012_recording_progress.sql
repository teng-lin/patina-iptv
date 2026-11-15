-- Playback progress belongs to retained recording media, not a provider URL.
CREATE TABLE recording_playback_progress (
    recording_id INTEGER PRIMARY KEY REFERENCES recording_jobs(id) ON DELETE CASCADE,
    position_ms INTEGER NOT NULL DEFAULT 0 CHECK(position_ms >= 0),
    duration_ms INTEGER CHECK(duration_ms IS NULL OR duration_ms >= 0),
    completed INTEGER NOT NULL DEFAULT 0 CHECK(completed IN (0,1)),
    updated_at INTEGER NOT NULL CHECK(updated_at >= 0)
);
