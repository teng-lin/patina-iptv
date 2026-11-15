-- Synthetic playback progress on a retained recording.
UPDATE recording_jobs SET status='completed',bytes=18800 WHERE id=41;
INSERT INTO recording_playback_progress(recording_id,position_ms,duration_ms,completed,updated_at)
VALUES(41,12000,60000,0,1800000020);
