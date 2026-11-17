-- A synced spool's intended terminal outcome, committed before its final rename.
-- Bound to immutable accepted intent and content; deleted jobs cannot be resurrected.
CREATE TABLE recording_completions (
    job_id INTEGER PRIMARY KEY REFERENCES recording_jobs(id) ON DELETE CASCADE,
    source_incarnation TEXT NOT NULL,
    outcome TEXT NOT NULL CHECK(outcome IN ('completed','partial','failed')),
    bytes INTEGER NOT NULL CHECK(bytes >= 0),
    digest TEXT NOT NULL,
    message TEXT NOT NULL
);
-- Another accepted transition supersedes any pending finalization receipt.
CREATE TRIGGER recording_completion_superseded AFTER UPDATE OF status ON recording_jobs
WHEN NEW.status <> OLD.status
BEGIN
    DELETE FROM recording_completions WHERE job_id=NEW.id;
END;
