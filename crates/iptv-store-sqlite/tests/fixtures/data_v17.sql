-- Generated out-of-order series progress keeps an explicit episode identity.
INSERT INTO vod_progress(source_id,kind,content_id,parent_id,position_ms,duration_ms,completed,updated_at)
SELECT source_id,'episode','generated-episode-five',id,37000,120000,0,1800000017 FROM series ORDER BY source_id,id LIMIT 1;
INSERT INTO vod_progress(source_id,kind,content_id,position_ms,duration_ms,completed,updated_at,last_episode_id)
SELECT source_id,'series',id,37000,120000,0,1800000017,'generated-episode-five' FROM series ORDER BY source_id,id LIMIT 1;
