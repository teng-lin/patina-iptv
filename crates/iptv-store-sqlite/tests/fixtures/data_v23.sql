-- Preserve the high water mark of the generated groups seeded by earlier fixtures.
UPDATE custom_group_sequence SET last_id=COALESCE((SELECT MAX(id) FROM custom_groups), 0);
