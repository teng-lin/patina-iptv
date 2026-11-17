-- Synthetic recording with a synced completion intent awaiting publication.
INSERT INTO recording_jobs(id,source_id,channel_id,channel_name,title,starts_at,ends_at,status,source_incarnation)
SELECT 42,source_id,channel_id,channel_name,title,starts_at+60,ends_at+60,'recording',source_incarnation
FROM recording_jobs WHERE id=41;
INSERT INTO recording_completions(job_id,source_incarnation,outcome,bytes,digest,message)
SELECT id,source_incarnation,'completed',188,'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',''
FROM recording_jobs WHERE id=42;
