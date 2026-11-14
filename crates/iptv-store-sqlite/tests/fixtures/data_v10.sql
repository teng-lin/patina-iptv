-- Synthetic semantic revision state from the guide-repair schema.
UPDATE guide_revisions SET mapping_revision = 7, configuration_revision = 3
WHERE source_id = 1;
