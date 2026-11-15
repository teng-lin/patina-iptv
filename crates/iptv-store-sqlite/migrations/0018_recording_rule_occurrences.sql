-- Different recurring rules may need the same captured programme. Keep each
-- rule's occurrence/cancellation authority while sharing one bounded job.
CREATE TABLE recording_rule_occurrences (
    job_id INTEGER NOT NULL,
    rule_id INTEGER NOT NULL REFERENCES recording_rules(id) ON DELETE CASCADE,
    starts_at INTEGER NOT NULL,
    PRIMARY KEY(rule_id, starts_at)
) WITHOUT ROWID;
INSERT INTO recording_rule_occurrences(job_id,rule_id,starts_at)
SELECT job_id,rule_id,starts_at FROM recording_rule_jobs;
DROP TABLE recording_rule_jobs;
ALTER TABLE recording_rule_occurrences RENAME TO recording_rule_jobs;
CREATE INDEX recording_rule_jobs_job ON recording_rule_jobs(job_id);
