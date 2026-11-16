-- Continue watching checks only the watched episodes of one source and series.
-- Cover the eligibility predicate too, so the planner chooses this parent lookup
-- without depending on ANALYZE statistics or visiting unrelated episode records.
CREATE INDEX vod_progress_by_parent
ON vod_progress(source_id, parent_id, completed, position_ms)
WHERE kind = 'episode';
