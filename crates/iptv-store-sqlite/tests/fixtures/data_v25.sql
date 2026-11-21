-- A generated recording canceled before it started, as v25 builds store it.
INSERT INTO recording_jobs(id,source_id,channel_id,channel_name,title,starts_at,ends_at,status,
    message,source_incarnation)
SELECT 44,source_id,channel_id,channel_name,title,starts_at+180,ends_at+180,'canceled','Canceled',
    source_incarnation
FROM recording_jobs WHERE id=41;
