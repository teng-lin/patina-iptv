-- Movies or series that failed to download while the rest of an import
-- succeeded (kind only), cleared by the next success.
ALTER TABLE source_imports ADD COLUMN movie_error_kind TEXT;
ALTER TABLE source_imports ADD COLUMN series_error_kind TEXT;
