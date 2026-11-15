-- Synthetic durable recurrence and its acknowledged occurrence.
INSERT INTO recording_rules(id,source_id,source_incarnation,channel_id,channel_name,title,starts_at,ends_at,local_start,repeat,enabled)
VALUES(71,1,(SELECT incarnation FROM guide_revisions WHERE source_id=1),'generated-channel','Generated channel','Generated programme',1800000000,1800000060,1800000000,'daily',1);
INSERT INTO recording_rule_jobs(job_id,rule_id,starts_at) VALUES(41,71,1800000000);
