-- US spelling for stored names (#139). Every row is kept: tables and
-- columns are renamed in place, indexes are dropped and created again
-- under their new names, and stored words are rewritten.
ALTER TABLE favourites RENAME TO favorites;
DROP INDEX favourites_by_position;
CREATE INDEX favorites_by_position ON favorites (source_id, position);

ALTER TABLE epg_programmes RENAME TO epg_programs;
DROP INDEX epg_programmes_by_stop;
CREATE INDEX epg_programs_by_stop ON epg_programs (stop);

ALTER TABLE epg_feeds RENAME COLUMN programme_count TO program_count;

-- Import failure kinds: the slug of a canceled import.
UPDATE source_imports SET last_error_kind = 'canceled' WHERE last_error_kind = 'cancelled';
UPDATE source_imports SET movie_error_kind = 'canceled' WHERE movie_error_kind = 'cancelled';
UPDATE source_imports SET series_error_kind = 'canceled' WHERE series_error_kind = 'cancelled';
UPDATE epg_guides SET last_error_kind = 'canceled' WHERE last_error_kind = 'cancelled';
UPDATE epg_feed_results SET error_kind = 'canceled' WHERE error_kind = 'cancelled';

-- Recording jobs: the status CHECK names 'canceled', so the table is built
-- again. Dropping a parent with foreign keys on deletes its children, so
-- both children are copied aside and dropped first, then everything is
-- created again with the same ids. A child row whose job is gone (which
-- foreign keys never allowed) is not copied back rather than failing.
CREATE TABLE us_spelling_jobs AS SELECT * FROM recording_jobs;
CREATE TABLE us_spelling_completions AS SELECT * FROM recording_completions;
CREATE TABLE us_spelling_progress AS SELECT * FROM recording_playback_progress;
DROP TRIGGER recording_completion_superseded;
DROP TABLE recording_completions;
DROP TABLE recording_playback_progress;
DROP TABLE recording_jobs;

CREATE TABLE recording_jobs (
    id INTEGER PRIMARY KEY,
    source_id INTEGER NOT NULL,
    channel_id TEXT NOT NULL,
    channel_name TEXT NOT NULL,
    title TEXT NOT NULL,
    starts_at INTEGER NOT NULL CHECK(starts_at >= 0),
    ends_at INTEGER NOT NULL CHECK(ends_at > starts_at AND ends_at - starts_at <= 86400),
    status TEXT NOT NULL CHECK(status IN
        ('scheduled','recording','completed','partial','failed','canceled','missed')),
    bytes INTEGER NOT NULL DEFAULT 0 CHECK(bytes >= 0),
    message TEXT NOT NULL DEFAULT '',
    source_incarnation TEXT NOT NULL
);
-- The stored message is an English interface string, translated when shown.
INSERT INTO recording_jobs
    (id, source_id, channel_id, channel_name, title, starts_at, ends_at, status, bytes,
     message, source_incarnation)
SELECT id, source_id, channel_id, channel_name, title, starts_at, ends_at,
    CASE status WHEN 'cancelled' THEN 'canceled' ELSE status END, bytes,
    CASE message WHEN 'Cancelled' THEN 'Canceled' ELSE message END, source_incarnation
FROM us_spelling_jobs;
CREATE INDEX recording_due ON recording_jobs(status, starts_at);
CREATE UNIQUE INDEX recording_unique_active ON recording_jobs(source_incarnation, source_id, channel_id, starts_at, ends_at)
    WHERE status IN ('scheduled','recording');
CREATE TRIGGER recording_completion_superseded AFTER UPDATE OF status ON recording_jobs
WHEN NEW.status <> OLD.status
BEGIN
    DELETE FROM recording_completions WHERE job_id=NEW.id;
END;

CREATE TABLE recording_completions (
    job_id INTEGER PRIMARY KEY REFERENCES recording_jobs(id) ON DELETE CASCADE,
    source_incarnation TEXT NOT NULL,
    outcome TEXT NOT NULL CHECK(outcome IN ('completed','partial','failed')),
    bytes INTEGER NOT NULL CHECK(bytes >= 0),
    digest TEXT NOT NULL,
    message TEXT NOT NULL
);
INSERT INTO recording_completions (job_id, source_incarnation, outcome, bytes, digest, message)
SELECT job_id, source_incarnation, outcome, bytes, digest, message FROM us_spelling_completions
WHERE job_id IN (SELECT id FROM recording_jobs);

CREATE TABLE recording_playback_progress (
    recording_id INTEGER PRIMARY KEY REFERENCES recording_jobs(id) ON DELETE CASCADE,
    position_ms INTEGER NOT NULL DEFAULT 0 CHECK(position_ms >= 0),
    duration_ms INTEGER CHECK(duration_ms IS NULL OR duration_ms >= 0),
    completed INTEGER NOT NULL DEFAULT 0 CHECK(completed IN (0,1)),
    updated_at INTEGER NOT NULL CHECK(updated_at >= 0)
);
INSERT INTO recording_playback_progress
    (recording_id, position_ms, duration_ms, completed, updated_at)
SELECT recording_id, position_ms, duration_ms, completed, updated_at FROM us_spelling_progress
WHERE recording_id IN (SELECT id FROM recording_jobs);

DROP TABLE us_spelling_jobs;
DROP TABLE us_spelling_completions;
DROP TABLE us_spelling_progress;
