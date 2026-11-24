-- Scheduled recording is retired. Nothing an earlier build scheduled may run,
-- whichever release the file comes from, and this runs before any recorder
-- reads the jobs. Rows stay, with their media and playback progress:
-- * every enabled recurring rule is disabled;
-- * every scheduled job without media is canceled;
-- * an interrupted capture with media, a recording row without a completion
--   receipt or a scheduled row resuming one, becomes a partial recording,
--   which the recorder's recovery finalizes instead of resuming. So does a
--   resuming row whose repeating rule was canceled: that left it canceled
--   with its bytes, and unplayable.
-- What was canceled is listed for the viewer until they have seen it.
CREATE TABLE retired_recording_schedules (
    id INTEGER PRIMARY KEY,
    channel_name TEXT NOT NULL,
    title TEXT NOT NULL,
    starts_at INTEGER NOT NULL,
    ends_at INTEGER NOT NULL,
    repeat TEXT NOT NULL CHECK(repeat IN ('once','daily','weekly','series'))
);
INSERT INTO retired_recording_schedules(channel_name,title,starts_at,ends_at,repeat)
SELECT channel_name,title,starts_at,ends_at,repeat FROM recording_rules
WHERE enabled=1 ORDER BY starts_at,id;
INSERT INTO retired_recording_schedules(channel_name,title,starts_at,ends_at,repeat)
SELECT channel_name,title,starts_at,ends_at,'once' FROM recording_jobs j
WHERE status='scheduled' AND bytes=0
AND NOT EXISTS(SELECT 1 FROM recording_rule_jobs r WHERE r.job_id=j.id)
ORDER BY starts_at,id;
UPDATE recording_rules SET enabled=0 WHERE enabled=1;
UPDATE recording_jobs SET status='canceled',message='Scheduled recording was removed'
WHERE status='scheduled' AND bytes=0;
UPDATE recording_jobs SET status='partial',message='Interrupted when the app closed'
WHERE status='scheduled'
OR (status='canceled' AND bytes>0)
OR (status='recording'
    AND NOT EXISTS(SELECT 1 FROM recording_completions c WHERE c.job_id=recording_jobs.id));
INSERT INTO store_meta(key,value)
SELECT 'retired_recording_notice','pending'
WHERE EXISTS(SELECT 1 FROM retired_recording_schedules);
