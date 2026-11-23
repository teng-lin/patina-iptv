-- Generated group choices saved by a schema 26 build.
INSERT INTO group_overlays(source_id, group_id, favorites_only, external_player, epg_offset_minutes)
VALUES (1, 'xt:live-group:1', 1, 0, 30)
ON CONFLICT(source_id, group_id) DO UPDATE SET
    favorites_only=excluded.favorites_only, epg_offset_minutes=excluded.epg_offset_minutes;
