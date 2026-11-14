-- Schema v10: stable catalogue/source and configured-feed incarnations,
-- semantic guide revisions, and durable inverse records for atomic repair.
-- No programme-row triggers: large imports remain bounded bulk publications.
CREATE TABLE guide_revisions (
    source_id INTEGER PRIMARY KEY NOT NULL REFERENCES sources (id) ON DELETE CASCADE,
    incarnation TEXT NOT NULL,
    mapping_revision INTEGER NOT NULL DEFAULT 0,
    configuration_revision INTEGER NOT NULL DEFAULT 0
);
INSERT INTO guide_revisions (source_id, incarnation)
SELECT id, lower(hex(randomblob(16))) FROM sources;
CREATE TABLE guide_feed_incarnations (
    source_id INTEGER NOT NULL REFERENCES sources (id) ON DELETE CASCADE,
    feed_id INTEGER NOT NULL,
    incarnation TEXT NOT NULL,
    PRIMARY KEY (source_id, feed_id)
) WITHOUT ROWID;
INSERT INTO guide_feed_incarnations (source_id, feed_id, incarnation)
SELECT source_id, feed_id, lower(hex(randomblob(16))) FROM guide_sources;
-- Existing cached rows acquire provenance only if their captured configuration
-- matches the configured feed. Later publications stamp within their transaction.
INSERT OR REPLACE INTO store_meta(key,value)
SELECT 'inspection.feed.' || g.source_id || '.' || g.feed_id, i.incarnation
FROM guide_sources g JOIN guide_feed_incarnations i
    ON i.source_id=g.source_id AND i.feed_id=g.feed_id
JOIN epg_feeds f ON f.source_id=g.source_id AND f.feed_id=g.feed_id
WHERE f.url=g.url AND f.shift_minutes=g.shift_minutes;
CREATE TRIGGER guide_source_created AFTER INSERT ON sources BEGIN
    INSERT INTO guide_revisions (source_id, incarnation)
    VALUES (NEW.id, lower(hex(randomblob(16))));
END;
CREATE TRIGGER guide_feed_created AFTER INSERT ON guide_sources BEGIN
    INSERT INTO guide_feed_incarnations (source_id, feed_id, incarnation)
    VALUES (NEW.source_id, NEW.feed_id, lower(hex(randomblob(16))));
    UPDATE guide_revisions SET configuration_revision = configuration_revision + 1
    WHERE source_id = NEW.source_id;
END;
CREATE TRIGGER guide_feed_removed AFTER DELETE ON guide_sources BEGIN
    DELETE FROM guide_feed_incarnations
    WHERE source_id = OLD.source_id AND feed_id = OLD.feed_id;
    UPDATE guide_revisions SET configuration_revision = configuration_revision + 1
    WHERE source_id = OLD.source_id;
END;
CREATE TRIGGER guide_feed_changed AFTER UPDATE OF provider_index, url, name, enabled,
    position, shift_minutes ON guide_sources
WHEN OLD.provider_index IS NOT NEW.provider_index OR OLD.url IS NOT NEW.url
    OR OLD.name IS NOT NEW.name OR OLD.enabled IS NOT NEW.enabled
    OR OLD.position IS NOT NEW.position OR OLD.shift_minutes IS NOT NEW.shift_minutes
BEGIN
    UPDATE guide_feed_incarnations SET incarnation = lower(hex(randomblob(16)))
    WHERE source_id = NEW.source_id AND feed_id = NEW.feed_id;
    UPDATE guide_revisions SET configuration_revision = configuration_revision + 1
    WHERE source_id = NEW.source_id;
END;
CREATE TRIGGER guide_mapping_created AFTER INSERT ON channel_overlays
WHEN NEW.epg_mode IS NOT NULL BEGIN
    UPDATE guide_revisions SET mapping_revision = mapping_revision + 1
    WHERE source_id = NEW.source_id;
END;
CREATE TRIGGER guide_mapping_changed AFTER UPDATE OF epg_mode, epg_source, epg_channel
    ON channel_overlays
WHEN OLD.epg_mode IS NOT NEW.epg_mode OR OLD.epg_source IS NOT NEW.epg_source
    OR OLD.epg_channel IS NOT NEW.epg_channel BEGIN
    UPDATE guide_revisions SET mapping_revision = mapping_revision + 1
    WHERE source_id = NEW.source_id;
END;
CREATE TRIGGER guide_mapping_removed AFTER DELETE ON channel_overlays
WHEN OLD.epg_mode IS NOT NULL BEGIN
    UPDATE guide_revisions SET mapping_revision = mapping_revision + 1
    WHERE source_id = OLD.source_id;
END;
CREATE TABLE guide_repair_commits (
    id TEXT PRIMARY KEY NOT NULL,
    source_id INTEGER NOT NULL REFERENCES sources (id) ON DELETE CASCADE,
    source_incarnation TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    undone_at INTEGER,
    payload TEXT NOT NULL
) WITHOUT ROWID;
CREATE INDEX guide_repair_commits_by_expiry ON guide_repair_commits (expires_at);
