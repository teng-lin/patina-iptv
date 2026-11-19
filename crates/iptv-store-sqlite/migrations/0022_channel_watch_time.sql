ALTER TABLE guide_sort RENAME TO guide_sort_legacy;

CREATE TABLE guide_sort (
    source_id INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    scope TEXT NOT NULL,
    sort TEXT NOT NULL CHECK(sort IN (
        'provider', 'name', 'name_desc', 'last_watched', 'oldest_watched', 'most_watched', 'least_watched'
    )),
    PRIMARY KEY(source_id, scope)
);

INSERT INTO guide_sort SELECT source_id, scope, sort FROM guide_sort_legacy;
DROP TABLE guide_sort_legacy;

CREATE TABLE channel_watch_time (
    source_id INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    channel_id TEXT NOT NULL,
    watched_ms INTEGER NOT NULL CHECK(watched_ms >= 0),
    PRIMARY KEY(source_id, channel_id)
);
