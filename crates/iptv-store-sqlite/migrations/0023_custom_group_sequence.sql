-- A deleted custom group's ID must never identify a replacement.
CREATE TABLE custom_group_sequence (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    last_id INTEGER NOT NULL
);
INSERT INTO custom_group_sequence(singleton, last_id)
SELECT 1, COALESCE(MAX(id), 0) FROM custom_groups;
