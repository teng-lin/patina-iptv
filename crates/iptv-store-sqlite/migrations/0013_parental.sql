-- Credentials remain inside SQLCipher and never enter settings or backups as plaintext.
CREATE TABLE parental_security (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    salt BLOB NOT NULL CHECK (length(salt) = 16),
    verifier BLOB NOT NULL CHECK (length(verifier) = 32),
    recovery_verifier BLOB NOT NULL CHECK (length(recovery_verifier) = 32),
    failures INTEGER NOT NULL DEFAULT 0 CHECK (failures >= 0),
    retry_after INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE parental_blocks (
    source_id INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    content_id TEXT NOT NULL,
    blocked INTEGER NOT NULL DEFAULT 1 CHECK (blocked IN (0,1)),
    PRIMARY KEY (source_id, kind, content_id)
) WITHOUT ROWID;
