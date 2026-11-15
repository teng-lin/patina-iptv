-- A series resumes the episode actually watched, including out-of-order viewing.
ALTER TABLE vod_progress ADD COLUMN last_episode_id TEXT;

-- Best effort for catalogues written before an explicit resume identity existed.
UPDATE vod_progress AS parent SET last_episode_id=(
    SELECT episode.content_id FROM vod_progress AS episode
    WHERE episode.source_id=parent.source_id AND episode.kind='episode'
      AND episode.parent_id=parent.content_id
    ORDER BY episode.updated_at DESC,episode.content_id DESC LIMIT 1
) WHERE parent.kind='series';
