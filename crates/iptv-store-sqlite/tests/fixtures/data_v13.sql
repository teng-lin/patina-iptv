-- Synthetic one-way credential records and source-qualified protection.
INSERT INTO parental_security(singleton,salt,verifier,recovery_verifier,failures,retry_after)
VALUES(1,zeroblob(16),zeroblob(32),zeroblob(32),2,0);
INSERT INTO parental_blocks(source_id,kind,content_id,blocked)
VALUES(1,'live','generated-channel',1);
