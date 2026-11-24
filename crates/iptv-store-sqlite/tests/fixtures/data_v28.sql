-- A generated schedule canceled when scheduled recording was retired, still
-- listed for the viewer.
INSERT INTO retired_recording_schedules(channel_name,title,starts_at,ends_at,repeat)
VALUES('Generated channel','Generated retired programme',1800000300,1800000360,'weekly');
INSERT OR REPLACE INTO store_meta(key,value) VALUES('retired_recording_notice','pending');
