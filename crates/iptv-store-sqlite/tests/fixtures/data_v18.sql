-- Synthetic shared occurrence: two recurring intents retain one captured job.
INSERT INTO recording_rules(id,source_id,source_incarnation,channel_id,channel_name,title,starts_at,ends_at,local_start,repeat,enabled)
SELECT 72,source_id,source_incarnation,channel_id,channel_name,title,starts_at,ends_at,local_start,'weekly',1 FROM recording_rules WHERE id=71;
INSERT INTO recording_rule_jobs(job_id,rule_id,starts_at) VALUES(41,72,1800000000);
