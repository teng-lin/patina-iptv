-- table channel_groups
CREATE TABLE channel_groups (
    source_id INTEGER NOT NULL REFERENCES sources (id) ON DELETE CASCADE,
    group_id TEXT NOT NULL,
    channel_id TEXT NOT NULL,
    position INTEGER NOT NULL,
    PRIMARY KEY (source_id, group_id, channel_id)
) WITHOUT ROWID;
-- table channel_overlays
CREATE TABLE channel_overlays (
    source_id INTEGER NOT NULL REFERENCES sources (id) ON DELETE CASCADE,
    channel_id TEXT NOT NULL,
    custom_name TEXT,
    custom_number INTEGER,
    hidden INTEGER NOT NULL DEFAULT 0,
    blocked INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL, epg_mode TEXT CHECK (epg_mode IN ('none', 'xmltv')), epg_source TEXT, epg_channel TEXT,
    PRIMARY KEY (source_id, channel_id)
) WITHOUT ROWID;
-- table channels
CREATE TABLE channels (
    source_id INTEGER NOT NULL REFERENCES sources (id) ON DELETE CASCADE,
    id TEXT NOT NULL,
    position INTEGER NOT NULL,
    provider_id TEXT,
    name TEXT NOT NULL,
    number INTEGER,
    logo TEXT,
    group_ids TEXT NOT NULL,
    stream TEXT NOT NULL,
    epg_id TEXT,
    epg_name TEXT,
    epg_shift_minutes INTEGER,
    catch_up TEXT,
    radio INTEGER NOT NULL DEFAULT 0,
    UNIQUE (source_id, id)
);
-- table custom_group_members
CREATE TABLE custom_group_members (
    group_id INTEGER NOT NULL REFERENCES custom_groups (id) ON DELETE CASCADE,
    channel_id TEXT NOT NULL,
    position INTEGER NOT NULL,
    PRIMARY KEY (group_id, channel_id)
) WITHOUT ROWID;
-- table custom_groups
CREATE TABLE custom_groups (
    id INTEGER PRIMARY KEY,
    source_id INTEGER NOT NULL REFERENCES sources (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    position INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);
-- table epg_bindings
CREATE TABLE epg_bindings (
    source_id INTEGER NOT NULL REFERENCES epg_guides (source_id) ON DELETE CASCADE,
    channel_key TEXT NOT NULL,
    epg_channel_id INTEGER NOT NULL REFERENCES epg_channels (id) ON DELETE CASCADE,
    match_kind TEXT NOT NULL,
    PRIMARY KEY (source_id, channel_key)
) WITHOUT ROWID;
-- table epg_channels
CREATE TABLE epg_channels (
    id INTEGER PRIMARY KEY,
    source_id INTEGER NOT NULL,
    feed_id INTEGER NOT NULL,
    epg_channel TEXT NOT NULL,
    UNIQUE (source_id, feed_id, epg_channel),
    FOREIGN KEY (source_id, feed_id) REFERENCES epg_feeds (source_id, feed_id) ON DELETE CASCADE
);
-- table epg_descriptions
CREATE TABLE epg_descriptions (
    channel_id INTEGER NOT NULL REFERENCES epg_channels (id) ON DELETE CASCADE,
    start INTEGER NOT NULL,
    description TEXT NOT NULL,
    PRIMARY KEY (channel_id, start)
) WITHOUT ROWID;
-- table epg_feed_channels
CREATE TABLE epg_feed_channels (
    source_id INTEGER NOT NULL,
    feed_id INTEGER NOT NULL,
    epg_channel TEXT NOT NULL,
    name TEXT,
    PRIMARY KEY (source_id, feed_id, epg_channel),
    FOREIGN KEY (source_id, feed_id) REFERENCES epg_feeds (source_id, feed_id) ON DELETE CASCADE
) WITHOUT ROWID;
-- table epg_feed_results
CREATE TABLE epg_feed_results (
    source_id INTEGER NOT NULL REFERENCES epg_guides (source_id) ON DELETE CASCADE,
    feed_id INTEGER NOT NULL,
    attempted_at INTEGER NOT NULL,
    error_kind TEXT,
    PRIMARY KEY (source_id, feed_id)
) WITHOUT ROWID;
-- table epg_feeds
CREATE TABLE epg_feeds (
    source_id INTEGER NOT NULL REFERENCES epg_guides (source_id) ON DELETE CASCADE,
    feed_id INTEGER NOT NULL,
    url TEXT NOT NULL,
    priority INTEGER NOT NULL,
    user_agent TEXT,
    assumed_offset_minutes INTEGER NOT NULL DEFAULT 0,
    shift_minutes INTEGER NOT NULL DEFAULT 0,
    store_descriptions INTEGER NOT NULL DEFAULT 1,
    past_days_to_keep INTEGER,
    fetched_at INTEGER NOT NULL,
    etag TEXT,
    last_modified TEXT,
    bytes INTEGER NOT NULL DEFAULT 0,
    channel_count INTEGER NOT NULL DEFAULT 0,
    programme_count INTEGER NOT NULL DEFAULT 0, content_supplied INTEGER NOT NULL DEFAULT 1 CHECK(content_supplied IN (0, 1)),
    PRIMARY KEY (source_id, feed_id)
) WITHOUT ROWID;
-- table epg_guides
CREATE TABLE epg_guides (
    source_id INTEGER PRIMARY KEY NOT NULL REFERENCES sources (id) ON DELETE CASCADE,
    replaced_at INTEGER,
    attempted_at INTEGER,
    last_error_kind TEXT,
    match_fingerprint INTEGER,
    bound_channels INTEGER NOT NULL DEFAULT 0,
    ends_at INTEGER
);
-- table epg_programmes
CREATE TABLE epg_programmes (
    channel_id INTEGER NOT NULL REFERENCES epg_channels (id) ON DELETE CASCADE,
    start INTEGER NOT NULL,
    stop INTEGER NOT NULL,
    title TEXT NOT NULL,
    sub_title TEXT,
    category TEXT,
    icon TEXT,
    rating TEXT,
    catchup_id TEXT,
    season INTEGER,
    episode INTEGER,
    flags INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (channel_id, start)
) WITHOUT ROWID;
-- table favourites
CREATE TABLE favourites (
    source_id INTEGER NOT NULL REFERENCES sources (id) ON DELETE CASCADE,
    channel_id TEXT NOT NULL,
    position INTEGER NOT NULL,
    added_at INTEGER NOT NULL,
    PRIMARY KEY (source_id, channel_id)
) WITHOUT ROWID;
-- table group_overlays
CREATE TABLE group_overlays (
    source_id INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    group_id TEXT NOT NULL,
    custom_name TEXT,
    hidden INTEGER NOT NULL DEFAULT 0 CHECK(hidden IN (0,1)),
    position INTEGER,
    PRIMARY KEY(source_id, group_id)
);
-- table groups
CREATE TABLE groups (
    source_id INTEGER NOT NULL REFERENCES sources (id) ON DELETE CASCADE,
    id TEXT NOT NULL,
    kind TEXT NOT NULL,
    position INTEGER NOT NULL,
    name TEXT NOT NULL,
    provider_id TEXT,
    adult INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (source_id, id)
) WITHOUT ROWID;
-- table guide_feed_incarnations
CREATE TABLE guide_feed_incarnations (
    source_id INTEGER NOT NULL REFERENCES sources (id) ON DELETE CASCADE,
    feed_id INTEGER NOT NULL,
    incarnation TEXT NOT NULL,
    PRIMARY KEY (source_id, feed_id)
) WITHOUT ROWID;
-- table guide_repair_commits
CREATE TABLE guide_repair_commits (
    id TEXT PRIMARY KEY NOT NULL,
    source_id INTEGER NOT NULL REFERENCES sources (id) ON DELETE CASCADE,
    source_incarnation TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    undone_at INTEGER,
    payload TEXT NOT NULL
) WITHOUT ROWID;
-- table guide_revisions
CREATE TABLE guide_revisions (
    source_id INTEGER PRIMARY KEY NOT NULL REFERENCES sources (id) ON DELETE CASCADE,
    incarnation TEXT NOT NULL,
    mapping_revision INTEGER NOT NULL DEFAULT 0,
    configuration_revision INTEGER NOT NULL DEFAULT 0
);
-- table guide_sources
CREATE TABLE guide_sources (
    source_id INTEGER NOT NULL REFERENCES sources (id) ON DELETE CASCADE,
    feed_id INTEGER NOT NULL,
    provider_index INTEGER,
    url TEXT NOT NULL,
    name TEXT,
    enabled INTEGER NOT NULL DEFAULT 1,
    position INTEGER NOT NULL,
    shift_minutes INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (source_id, feed_id),
    UNIQUE (source_id, provider_index)
) WITHOUT ROWID;
-- table manual_order
CREATE TABLE manual_order (
    source_id INTEGER NOT NULL REFERENCES sources (id) ON DELETE CASCADE,
    scope TEXT NOT NULL,
    channel_id TEXT NOT NULL,
    position INTEGER NOT NULL,
    PRIMARY KEY (source_id, scope, channel_id)
) WITHOUT ROWID;
-- table parental_blocks
CREATE TABLE parental_blocks (
    source_id INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    content_id TEXT NOT NULL,
    blocked INTEGER NOT NULL DEFAULT 1 CHECK (blocked IN (0,1)),
    PRIMARY KEY (source_id, kind, content_id)
) WITHOUT ROWID;
-- table parental_security
CREATE TABLE parental_security (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    salt BLOB NOT NULL CHECK (length(salt) = 16),
    verifier BLOB NOT NULL CHECK (length(verifier) = 32),
    recovery_verifier BLOB NOT NULL CHECK (length(recovery_verifier) = 32),
    failures INTEGER NOT NULL DEFAULT 0 CHECK (failures >= 0),
    retry_after INTEGER NOT NULL DEFAULT 0
);
-- table recording_jobs
CREATE TABLE recording_jobs (
    id INTEGER PRIMARY KEY,
    source_id INTEGER NOT NULL,
    channel_id TEXT NOT NULL,
    channel_name TEXT NOT NULL,
    title TEXT NOT NULL,
    starts_at INTEGER NOT NULL CHECK(starts_at >= 0),
    ends_at INTEGER NOT NULL CHECK(ends_at > starts_at AND ends_at - starts_at <= 86400),
    status TEXT NOT NULL CHECK(status IN
        ('scheduled','recording','completed','partial','failed','cancelled','missed')),
    bytes INTEGER NOT NULL DEFAULT 0 CHECK(bytes >= 0),
    message TEXT NOT NULL DEFAULT '',
    source_incarnation TEXT NOT NULL
);
-- table recording_playback_progress
CREATE TABLE recording_playback_progress (
    recording_id INTEGER PRIMARY KEY REFERENCES recording_jobs(id) ON DELETE CASCADE,
    position_ms INTEGER NOT NULL DEFAULT 0 CHECK(position_ms >= 0),
    duration_ms INTEGER CHECK(duration_ms IS NULL OR duration_ms >= 0),
    completed INTEGER NOT NULL DEFAULT 0 CHECK(completed IN (0,1)),
    updated_at INTEGER NOT NULL CHECK(updated_at >= 0)
);
-- table recording_rule_jobs
CREATE TABLE "recording_rule_jobs" (
    job_id INTEGER NOT NULL,
    rule_id INTEGER NOT NULL REFERENCES recording_rules(id) ON DELETE CASCADE,
    starts_at INTEGER NOT NULL,
    PRIMARY KEY(rule_id, starts_at)
) WITHOUT ROWID;
-- table recording_rules
CREATE TABLE recording_rules (
    id INTEGER PRIMARY KEY,
    source_id INTEGER NOT NULL,
    source_incarnation TEXT NOT NULL,
    channel_id TEXT NOT NULL,
    channel_name TEXT NOT NULL,
    title TEXT NOT NULL,
    starts_at INTEGER NOT NULL,
    ends_at INTEGER NOT NULL,
    local_start INTEGER NOT NULL,
    repeat TEXT NOT NULL CHECK(repeat IN ('daily','weekly','series')),
    enabled INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0,1))
, lead_seconds INTEGER NOT NULL DEFAULT 0 CHECK(lead_seconds BETWEEN 0 AND 3600), trail_seconds INTEGER NOT NULL DEFAULT 0 CHECK(trail_seconds BETWEEN 0 AND 3600));
-- table schema_migrations
CREATE TABLE schema_migrations (
    version INTEGER PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    applied_at INTEGER NOT NULL
);
-- table search_history
CREATE TABLE search_history (
    position INTEGER PRIMARY KEY NOT NULL CHECK(position >= 0),
    query TEXT NOT NULL
);
-- table series
CREATE TABLE series (
    source_id INTEGER NOT NULL REFERENCES sources (id) ON DELETE CASCADE,
    id TEXT NOT NULL,
    position INTEGER NOT NULL,
    provider_id TEXT,
    name TEXT NOT NULL,
    group_ids TEXT NOT NULL,
    poster TEXT,
    plot TEXT,
    year INTEGER,
    rating_x10 INTEGER,
    last_modified INTEGER,
    PRIMARY KEY (source_id, id)
) WITHOUT ROWID;
-- table series_groups
CREATE TABLE series_groups (
    source_id INTEGER NOT NULL REFERENCES sources (id) ON DELETE CASCADE,
    group_id TEXT NOT NULL,
    series_id TEXT NOT NULL,
    position INTEGER NOT NULL,
    PRIMARY KEY (source_id, group_id, series_id)
) WITHOUT ROWID;
-- table source_credentials
CREATE TABLE source_credentials (
    source_id INTEGER NOT NULL REFERENCES sources (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (source_id, name)
) WITHOUT ROWID;
-- table source_imports
CREATE TABLE source_imports (
    source_id INTEGER PRIMARY KEY NOT NULL REFERENCES sources (id) ON DELETE CASCADE,
    last_attempt_at INTEGER,
    last_success_at INTEGER,
    group_count INTEGER NOT NULL DEFAULT 0,
    channel_count INTEGER NOT NULL DEFAULT 0,
    vod_count INTEGER NOT NULL DEFAULT 0,
    series_count INTEGER NOT NULL DEFAULT 0,
    duplicate_count INTEGER NOT NULL DEFAULT 0,
    last_error_kind TEXT,
    last_error_at INTEGER
);
-- table source_policy
CREATE TABLE source_policy (
    source_id INTEGER PRIMARY KEY REFERENCES sources(id) ON DELETE CASCADE,
    enabled INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0,1)),
    refresh_seconds INTEGER NOT NULL DEFAULT 0 CHECK(refresh_seconds >= 0),
    refresh_on_start INTEGER NOT NULL DEFAULT 1 CHECK(refresh_on_start IN (0,1))
);
-- table source_settings
CREATE TABLE source_settings (
    source_id INTEGER PRIMARY KEY NOT NULL REFERENCES sources (id) ON DELETE CASCADE,
    epg_url TEXT,
    updated_at INTEGER NOT NULL
);
-- table sources
CREATE TABLE sources (
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('m3u', 'xtream', 'stalker')),
    name TEXT NOT NULL,
    config TEXT NOT NULL,
    position INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
-- table store_meta
CREATE TABLE store_meta (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
) WITHOUT ROWID;
-- table vod
CREATE TABLE vod (
    source_id INTEGER NOT NULL REFERENCES sources (id) ON DELETE CASCADE,
    id TEXT NOT NULL,
    position INTEGER NOT NULL,
    provider_id TEXT,
    name TEXT NOT NULL,
    group_ids TEXT NOT NULL,
    poster TEXT,
    plot TEXT,
    year INTEGER,
    rating_x10 INTEGER,
    added INTEGER,
    duration_secs INTEGER,
    stream TEXT NOT NULL,
    is_series INTEGER NOT NULL DEFAULT 0, episode_numbers TEXT NOT NULL DEFAULT '[]',
    PRIMARY KEY (source_id, id)
) WITHOUT ROWID;
-- table vod_groups
CREATE TABLE vod_groups (
    source_id INTEGER NOT NULL REFERENCES sources (id) ON DELETE CASCADE,
    group_id TEXT NOT NULL,
    vod_id TEXT NOT NULL,
    position INTEGER NOT NULL,
    PRIMARY KEY (source_id, group_id, vod_id)
) WITHOUT ROWID;
-- table vod_progress
CREATE TABLE vod_progress (
    source_id INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK(kind IN ('movie','series','episode')),
    content_id TEXT NOT NULL,
    parent_id TEXT,
    position_ms INTEGER NOT NULL DEFAULT 0 CHECK(position_ms >= 0),
    duration_ms INTEGER CHECK(duration_ms >= 0),
    completed INTEGER NOT NULL DEFAULT 0 CHECK(completed IN (0,1)),
    updated_at INTEGER NOT NULL, last_episode_id TEXT,
    PRIMARY KEY(source_id,kind,content_id)
) WITHOUT ROWID;
-- table watch_history
CREATE TABLE watch_history (
    id INTEGER PRIMARY KEY,
    source_id INTEGER NOT NULL REFERENCES sources (id) ON DELETE CASCADE,
    item_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    watched_at INTEGER NOT NULL,
    position_secs INTEGER
);
-- index channel_groups_by_position
CREATE INDEX channel_groups_by_position ON channel_groups (source_id, group_id, position);
-- index channels_by_position
CREATE INDEX channels_by_position ON channels (source_id, position);
-- index custom_group_members_by_position
CREATE INDEX custom_group_members_by_position ON custom_group_members (group_id, position);
-- index custom_groups_by_position
CREATE INDEX custom_groups_by_position ON custom_groups (source_id, position);
-- index epg_bindings_by_channel
CREATE INDEX epg_bindings_by_channel ON epg_bindings (epg_channel_id);
-- index epg_programmes_by_stop
CREATE INDEX epg_programmes_by_stop ON epg_programmes (stop);
-- index favourites_by_position
CREATE INDEX favourites_by_position ON favourites (source_id, position);
-- index groups_by_position
CREATE INDEX groups_by_position ON groups (source_id, kind, position);
-- index guide_repair_commits_by_expiry
CREATE INDEX guide_repair_commits_by_expiry ON guide_repair_commits (expires_at);
-- index manual_order_by_position
CREATE INDEX manual_order_by_position ON manual_order (source_id, scope, position);
-- index recording_due
CREATE INDEX recording_due ON recording_jobs(status, starts_at);
-- index recording_rule_jobs_job
CREATE INDEX recording_rule_jobs_job ON recording_rule_jobs(job_id);
-- index recording_unique_active
CREATE UNIQUE INDEX recording_unique_active ON recording_jobs(source_incarnation, source_id, channel_id, starts_at, ends_at)
    WHERE status IN ('scheduled','recording');
-- index series_by_position
CREATE INDEX series_by_position ON series (source_id, position);
-- index series_groups_by_position
CREATE INDEX series_groups_by_position ON series_groups (source_id, group_id, position);
-- index vod_by_position
CREATE INDEX vod_by_position ON vod (source_id, position);
-- index vod_groups_by_position
CREATE INDEX vod_groups_by_position ON vod_groups (source_id, group_id, position);
-- index vod_progress_recent
CREATE INDEX vod_progress_recent ON vod_progress(updated_at DESC);
-- index watch_history_by_item
CREATE INDEX watch_history_by_item ON watch_history (source_id, item_id, watched_at);
-- index watch_history_by_time
CREATE INDEX watch_history_by_time ON watch_history (source_id, watched_at);
-- trigger guide_feed_changed
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
-- trigger guide_feed_created
CREATE TRIGGER guide_feed_created AFTER INSERT ON guide_sources BEGIN
    INSERT INTO guide_feed_incarnations (source_id, feed_id, incarnation)
    VALUES (NEW.source_id, NEW.feed_id, lower(hex(randomblob(16))));
    UPDATE guide_revisions SET configuration_revision = configuration_revision + 1
    WHERE source_id = NEW.source_id;
END;
-- trigger guide_feed_removed
CREATE TRIGGER guide_feed_removed AFTER DELETE ON guide_sources BEGIN
    DELETE FROM guide_feed_incarnations
    WHERE source_id = OLD.source_id AND feed_id = OLD.feed_id;
    UPDATE guide_revisions SET configuration_revision = configuration_revision + 1
    WHERE source_id = OLD.source_id;
END;
-- trigger guide_mapping_changed
CREATE TRIGGER guide_mapping_changed AFTER UPDATE OF epg_mode, epg_source, epg_channel
    ON channel_overlays
WHEN OLD.epg_mode IS NOT NEW.epg_mode OR OLD.epg_source IS NOT NEW.epg_source
    OR OLD.epg_channel IS NOT NEW.epg_channel BEGIN
    UPDATE guide_revisions SET mapping_revision = mapping_revision + 1
    WHERE source_id = NEW.source_id;
END;
-- trigger guide_mapping_created
CREATE TRIGGER guide_mapping_created AFTER INSERT ON channel_overlays
WHEN NEW.epg_mode IS NOT NULL BEGIN
    UPDATE guide_revisions SET mapping_revision = mapping_revision + 1
    WHERE source_id = NEW.source_id;
END;
-- trigger guide_mapping_removed
CREATE TRIGGER guide_mapping_removed AFTER DELETE ON channel_overlays
WHEN OLD.epg_mode IS NOT NULL BEGIN
    UPDATE guide_revisions SET mapping_revision = mapping_revision + 1
    WHERE source_id = OLD.source_id;
END;
-- trigger guide_source_created
CREATE TRIGGER guide_source_created AFTER INSERT ON sources BEGIN
    INSERT INTO guide_revisions (source_id, incarnation)
    VALUES (NEW.id, lower(hex(randomblob(16))));
END;
