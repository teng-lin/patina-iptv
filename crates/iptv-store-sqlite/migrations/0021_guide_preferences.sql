CREATE TABLE guide_sort (
    source_id INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    scope TEXT NOT NULL,
    sort TEXT NOT NULL CHECK(sort IN ('provider','name')),
    PRIMARY KEY(source_id, scope)
);

CREATE TABLE guide_blocks (
    source_id INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK(kind IN ('channel','group')),
    content_id TEXT NOT NULL,
    PRIMARY KEY(source_id, kind, content_id)
);
