ALTER TABLE group_overlays ADD COLUMN favorites_only INTEGER NOT NULL DEFAULT 0 CHECK (favorites_only IN (0, 1));
ALTER TABLE group_overlays ADD COLUMN external_player INTEGER NOT NULL DEFAULT 0 CHECK (external_player IN (0, 1));
ALTER TABLE group_overlays ADD COLUMN epg_offset_minutes INTEGER NOT NULL DEFAULT 0 CHECK (epg_offset_minutes BETWEEN -720 AND 840);
