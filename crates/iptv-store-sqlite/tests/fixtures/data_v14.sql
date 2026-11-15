-- Generated watch progress exercises encrypted migrations and portable export.
INSERT INTO vod_progress(source_id,kind,content_id,position_ms,duration_ms,completed,updated_at)
SELECT source_id,CASE WHEN is_series=1 THEN 'series' ELSE 'movie' END,id,15000,120000,0,1800000000 FROM vod ORDER BY source_id,id LIMIT 1;
UPDATE vod SET episode_numbers='[2,4]' WHERE is_series=1;
