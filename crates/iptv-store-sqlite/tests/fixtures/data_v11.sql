-- Synthetic host-local recording intent.
INSERT INTO recording_jobs(id,source_id,channel_id,channel_name,title,starts_at,ends_at,status,source_incarnation)
VALUES(41,1,'generated-channel','Generated channel','Generated programme',1800000000,1800000060,'scheduled',
    (SELECT incarnation FROM guide_revisions WHERE source_id=1));
