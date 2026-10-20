-- Opaque issuance state is bounded and owned; no prompt, signature or encrypted blob is stored.
CREATE TABLE protocol_sessions (
    key TEXT PRIMARY KEY NOT NULL,
    pin_json TEXT NOT NULL,
    last_used_ms INTEGER NOT NULL
);
CREATE INDEX protocol_sessions_last_used ON protocol_sessions(last_used_ms);
CREATE TABLE protocol_artifacts (
    owner TEXT NOT NULL REFERENCES protocol_sessions(key) ON DELETE CASCADE,
    key_digest TEXT NOT NULL CHECK(length(key_digest)=64),
    issued_ms INTEGER NOT NULL,
    last_used_ms INTEGER NOT NULL,
    PRIMARY KEY(owner,key_digest)
);
CREATE INDEX protocol_artifacts_last_used ON protocol_artifacts(last_used_ms);
-- Legacy pins can be retained. Legacy artifact hashes lacked owner metadata and cannot
-- be safely assigned to a session, so they are deliberately invalidated (fail closed).
INSERT INTO protocol_sessions(key,pin_json,last_used_ms)
SELECT key,value,CAST(strftime('%s','now') AS INTEGER)*1000 FROM settings
WHERE key LIKE 'protocol_session:%';
DELETE FROM settings WHERE key LIKE 'protocol_session:%' OR key LIKE 'protocol_artifact:%';
